# E10--npc-equipment-sets

Brief · resumable · draft, owner rulings of 2026-11-23 applied; owner API review pending · Epic 10 · reads: `context/lib/entity_model.md` §2 (Weapon vocabulary, Weapon mounts, Defense layers and damage types), §4, §7c · `context/lib/scripting.md` §1 (Authoring API style), §2, §11, §12 · `context/lib/networking.md` §Weapon placement is content, §Version gates, engine-randomness posture · `context/lib/resource_management.md` §7 (Sockets) · depends on `E21--character-weapon-mounts` (done) · read at bb7323fc5

## Problem
Owner-requested capability (2026-11-13 roadmap item "Weapon-gated enemy attacks"; confirmed 2026-11-22): a modder or level designer equips individual NPCs of one archetype differently, and equipment can change during play. One grunt carries a burst rifle, the next a marksman rifle and armor. Today an enemy's weapons are fixed in its archetype's attack rows, its armor in its `health` block, its visible gun in `mesh.attachments`, and its shots leave the top of its hitbox rather than that gun. No placement key, spawner key or script step varies any of them, so each combination needs its own archetype and map classname, a spawner releases only one, and nothing can change what an NPC carries at runtime. When done:
- Each NPC instance carries its own weapons, mounted attachments and defense layer capacities.
- An attack is available only while its weapon is carried. The carried weapon's mount shows on the NPC's rig, and its shots leave that weapon's muzzle.
- An archetype declares named equipment sets as authoring bundles, and a placement, spawner or script equips one.
- Co-op clients see what each NPC actually carries.

## Decisions
- **Carried equipment is the runtime state; a set is how it is chosen.** An instance's state is its carried weapons, its mounted attachments and its layer stack. The set it was equipped from does not survive spawn or `equip`. The gate, the guard and the wire read carried equipment only. So later single-weapon give, take, drop and pickup are additive: no change to the gate, guard or wire. "Carried" is named apart from held ownership (`entity_model.md` §3) and held enemies (§7c).
- **Weapons are referenced by identity.** Every weapon reference this brief adds or touches takes the weapon archetype's `defineEntity` value, as `inventory.loadout` does: set weapons, mount keys, the guard, and `defineAttack`'s `weapon`. The string form of `defineAttack`'s `weapon` is retired, and `content/` call sites migrate in this brief (pre-stable, `index.md`). Map keys stay strings, as the projectile trap's `weapon` key does.
- **Equipment set is a descriptor-owned authoring bundle.** `defineEquipment` returns a handle named by its binding, as defense layers are (`scripting.md` §1, Handles over strings). It names weapons, optional non-weapon gear (socket → prop model), and capacity overrides for the archetype's defense layers. Sets are rig-independent and shareable across archetypes. An archetype lists its sets; the list needs no `defineMod` registration. A placement selects a declared set and never tunes one (`entity_model.md` §4).
- **Weapon mounts come from `E21--character-weapon-mounts`.** That brief introduces the character-owned `mesh.weaponMounts` table: each weapon a character may wield, mapped to the mount baked for its rig. Its first consumer is the player avatar. This brief consumes the table with the shape that brief settles, and adds only the NPC parts:
  - A carried weapon mounts; one that is not carried does not.
  - The mounted list composes in order, later entries winning per socket: base `mesh.attachments`, then mounts of carried weapons, then set gear. Two carried weapons share a socket only when their entries name the same model there: one gun firing two ways, as the limitator's burst and long rifles on one AR_4 `hand_r` (Load rules). Set gear carries the rig caveat, and the docs say so.
  - An entry may name `muzzle`, a socket on the body model, for a weapon built into the body: `{ weapon: armCannon, muzzle: "cannon_tip" }`. Such an entry needs no `attachments` (Load rules). The owner chose this shape because it avoids a precedent mutually exclusive with melee weapons, which later briefs expand on.
  - The limitator and the dev rifle squads move from `mesh.attachments` to `mesh.weaponMounts`; that brief has already moved their bakes into `models/ar_4/`.
- **NPC shots leave the carried weapon's muzzle.** An NPC's weapon shot launches from the posed `muzzle` socket of the model mounted for that weapon: the rig's holder joint × the rig bake × the bake's rigid `muzzle` socket. It passes through the obstructed-muzzle fallback and convergence players use (`resolve_muzzle_launch_origin`) and aims at the target from there. The AI eye stays the line-of-sight origin.
  - Where the weapon's entry names `muzzle`, the shot launches from that posed body socket instead.
  - This reverses E21's "Gameplay never reads a mount" (`networking.md` §Weapon placement is content) for NPCs, whose shots only the host computes, so no peer reproduces them from placement. Pawns keep placement plus `muzzleOffset`. Evidence: `research.md` §NPC shot origin.
  - No rig bake carries a `muzzle` socket today; the cyberpunk viewmodels do. This brief adds one to each AR_4 rig bake.
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
- **Hot reload never re-equips.** Carried weapons, the mounted list and the layer stack stay as worn, as a live player loadout does (`done/E16--wieldable-switching-inventory/`). Weapon descriptor edits reach carried weapons as today. Set edits reach the next spawn or `equip`. Mount and gear model paths follow the visual-asset deferral (`defer_visual_asset_descriptor_refreshes`) and apply at the next level load. A base-descriptor reload rebuilds the layer stack under the existing match-by-name rule, then reapplies the instance's capacity overrides. A carried weapon the reload removes leaves its attacks unavailable.
- **Load rules (both runtimes, twin behavior).** Each of these rejects load:
  - an empty list, a duplicated set, or a weight that is not finite and ≥ 0;
  - a list whose weights are all 0;
  - a capacity override naming a layer the archetype does not declare, a layer whose base capacity is an expression, or a capacity that is not a finite literal ≥ 0 (the rejection names the set and the layer);
  - a weapon-mount entry for a weapon none of the archetype's sets lists;
  - two carried weapons whose mount entries put different models on one socket. Under a set the archetype lists, the rejection names the set, the archetype and the socket. On an archetype without `equipment`, it names the archetype and the socket;
  - a weapon-mount entry that omits `attachments` and names no `muzzle`. This narrows E21's empty-attachments rule (`entity_model.md` §2 Weapon mounts): an attachment map that is present and empty still rejects.

  `equipment` on a descriptor without `behavior` warns, and the block drops. A set weapon that is not a projectile weapon is disabled with the existing enemy-attack warning. On an archetype that declares `mesh.weaponMounts`, a set weapon without an entry warns once per archetype and weapon at load and mounts nothing (E21's rule, `entity_model.md` §2 Weapon mounts); an archetype with no table stays silent.
- **Non-goals.**
  - Single-weapon give, take, drop on death, NPC pickup and scavenging: later Epic 10 work, additive on carried state; each weapon brings its own mount.
  - Off-hand IK onto a weapon grip socket. Mounts anchor at one socket, and two-handed clips place the off hand. A weapon model may already carry a `grip_l` socket, so nothing here blocks later IK.
  - Converting other string weapon references (player-side, map keys) to handles.
  - Hitscan enemy weapons: the projectile-only enemy rule stands.
  - Two of one weapon (dual wield): weapon state is keyed by weapon.
  - Per-set layer kinds, recharge, resist, health max, model or animations. Widening overrides later breaks no content.
  - Generating FGD `choices` from descriptors: the FGD stays hand-written, and the key is a string.
  - Player use: player pawns keep `inventory.loadout`.
  - One weapon for players and NPCs. NPCs keep `enemy_rifle*`. Sharing a weapon needs a per-use descriptor override the SDK lacks: spreading a weapon's components derives a new weapon identity. `drafts/descriptor-instance-overrides/` owns it.
  - Dismembering a built-in weapon, the body part that carries its muzzle: a later brief.
  - Consolidating two weapons that share one model into one modal weapon: `drafts/E16--persistent-firing-modes/` owns it, and moves the limitator and the rifle squads to one modal rifle.
  - Replicating NPC weapon resource state.
- **Docs amended at promotion:**
  - `entity_model.md`: Components, Weapon vocabulary (identity references), Weapon mounts (NPCs mount carried weapons; an NPC shot reads the mount's muzzle or the entry's body-socket `muzzle`; carried weapons share a socket only on one model; the load rules change, so an entry without `attachments` must name `muzzle`), Defense layers (set capacity overrides), §4 equipment selection, §7c gate and per-set check.
  - `scripting.md`: §2, §11, §12 group steps.
  - `networking.md`: §Weapon placement is content (NPC fire origin reads the mount), NPC mount list, §Version gates history, and the randomness posture (set draws ride the enemy behavior-draw exception and run at spawn).
  - `build_pipeline.md`: §Custom FGD.
  - Human-facing: `docs/scripting-reference.md`, `docs/level_design.md`, `sdk/TrenchBroom/postretro.fgd`.

### Scripting surface
Owner reviews this surface personally before promotion.
```ts
import { brain, carries, defineAttack, defineEntity, defineEquipment, defineReaction, npcs } from "postretro";
import { armor } from "./defenses";
import { enemyRifleBurstEntity as burstRifle, enemyRifleLongEntity as longRifle } from "./enemy-rifle";
import { armCannonEntity as armCannon } from "./arm-cannon";

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
    mesh: {
      model: "models/grunt/model.gltf",
      // Shape owned by E21--character-weapon-mounts. Baked against this rig; mounted only while carried.
      // Both rifles show one AR_4 bake, so a set carrying both may share `hand_r`.
      weaponMounts: [
        { weapon: burstRifle, attachments: { hand_r: "models/ar_4/grunt.gltf" } },
        { weapon: longRifle, attachments: { hand_r: "models/ar_4/grunt.gltf" } },
      ],
      // animations…
    },
    health: { max: 100, layers: [{ layer: armor, capacity: 0 }] }, // present, skipped until a set raises it
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

// A weapon built into the body: no prop; its shots leave a socket on the body model.
export const sentry = defineEntity({
  canonicalName: "sentry",
  components: {
    mesh: {
      model: "models/sentry/model.gltf",
      weaponMounts: [{ weapon: armCannon, muzzle: "cannon_tip" }],
      // animations…
    },
    // health, behavior with an attack on armCannon…
  },
});
```

### Boundary inventory
Both runtimes ship.

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| set declaration | descriptor equipment table | `"equipment"` (list of `{set, weight}`) | `defineEquipment`, `components.equipment` | `defineEquipment`, `components.equipment` | n/a |
| set name | binding name | string | binding name | binding name | `equipment` on `AiEnemy` and `entity_spawner` |
| weapon reference | canonical name | canonical name string | weapon `defineEntity` value | weapon `defineEntity` value | n/a |
| weapon mounts | defined by `E21--character-weapon-mounts` | as defined there | as defined there | as defined there | n/a |
| built-in muzzle | optional body socket on a weapon-mount entry | `"muzzle"` inside a `weaponMounts` entry | `muzzle` | `muzzle` | n/a |
| set gear | set attachment map | `"gear"` | `gear` | `gear` | n/a |
| guard read | brain scope, name-keyed leaf | `@carries.<weapon>` | `carries(weapon)` | `carries(weapon)` | n/a |
| group step | typed command | n/a | `npcs(…).equip(set)` | `npcs(…).equip(set)` | n/a |
| NPC mount list | snapshot record, optional | see Wire format | n/a | n/a | n/a |

### Wire format
NPC mount list on the snapshot entity record, bitcode like the record's other fields, mirroring `active_weapon_archetype`'s presence-flag shape:
- A presence flag. Absent: the client draws descriptor attachments.
- Present: a list of `(socket, model)` string pairs, sorted by socket, with no duplicate sockets. An empty list mounts nothing.
- Valid only on records whose class is `descriptor:`. A pawn or projectile record carrying it is rejected. The `active_weapon_archetype` validator, which rejects misplaced metadata, is the precedent.

## Acceptance
### Automated
**Per-instance equipment**
- [ ] Two placements of one archetype, keyed to different sets: each fires only the weapons it carries, has its set's layer capacities, and mounts the composed list of base attachments, then carried weapons' mounts, then set gear.
- [ ] One set listed by two archetypes on different rigs mounts each archetype's own bake for the same weapon.
- [ ] A set without `layers` keeps the base capacities. A set that raises a capacity-0 layer makes it drain before health. An instance whose set leaves it at 0 takes every hit on health.
- [ ] An archetype without `equipment` fires every attack weapon and loads as today.

**Shot origin**
- [ ] An NPC's shot launches from its carried weapon's posed muzzle, and the origin moves with the clip pose. Its line-of-sight and fire-gate tests read the eye, not the muzzle.
- [ ] An archetype whose entry names `muzzle` and no `attachments` loads in both runtimes and mounts nothing for that weapon; its shots launch from the named body socket and move with the clip pose. An entry with neither `attachments` nor `muzzle` rejects load in both runtimes, naming the character.
- [ ] A muzzle behind world geometry, with the eye's view clear, launches the shot from the eye, as for a player. An unobstructed muzzle launches from the muzzle and converges on the target.
- [ ] Each AR_4 rig bake carries a `muzzle` socket, and the limitator and each rifle squad launch from it.

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
- [ ] Hot reload leaves carried weapons, mounts and layer stacks as worn. A weapon descriptor edit reaches a carried weapon. A set edit reaches the next spawn and the next equip, not live instances; a mount or gear model edit applies at the next level load. A base-descriptor reload that edits a layer's recharge reaches equipped instances, and their capacity overrides stay applied.
- [ ] Each load rule in Decisions holds in both runtimes: empty list, duplicate set, bad weight, all-zero weights, override of an undeclared layer, override of an expression-capacity layer, bad override capacity, mount for an unlisted weapon, two carried weapons' mounts putting different models on one socket, a mount entry with neither `attachments` nor `muzzle`, `equipment` without `behavior`, non-projectile set weapon.
- [ ] A set whose weapons' mounts put different models on one socket, on an archetype listing it, rejects load, naming the set, the archetype and the socket. The same set loads on an archetype whose mounts for those weapons use different sockets, or name the same model on the shared socket. Set gear on a mounted weapon's socket still wins over the mount.
- [ ] An archetype without `equipment` whose attack weapons' mounts put different models on one socket rejects load, naming the archetype and the socket. The limitator and each rifle squad, whose burst and long rifles name one AR_4 bake on `hand_r`, load, and shots from either rifle launch from that bake's muzzle.
- [ ] On an archetype with `mesh.weaponMounts`, a set weapon without an entry warns once at load, naming the archetype and the weapon, and mounts nothing; a later spawn or equip carrying it does not warn again. An archetype without a table carries the same set silently.
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
- [ ] On a dev map, a rifle trooper's shots visibly leave its gun's barrel, not the space above its head, through every clip it fires in.
- [ ] Two-client co-op at 150 ms RTT (`POSTRETRO_NET_LAG`): the client sees each NPC's host-mounted weapon, before and after an equip.

## Path
- Seams:
  - Attacks: `AttackParams.weapon` → `resolve_projectile_attacks` (already per entity, in `BrainEntityPrograms`). Filter it by carried weapons.
  - Row skip: `RowSelection::skips` in `graph_eval.rs`, used by `first_enabled_target`. It sees the row and its direct target activity (`envelope.activities.get(&row.to)`), not nested descendants. `TurnGate` sits beside it.
  - Cancellation: `arbitrate_attack_turns` cancels only an attack that leaves the graph. A weapon that stops resolving is dropped in `AiWeaponControllers::sync` (`retain`, clearing `active`). `sync`'s early exit keys on graph `Arc::ptr_eq` and `descriptor_generation`; it must also key on carried equipment, or an `equip` never re-syncs.
  - Weapon state: `AiWeaponControllers::sync` derives it from resolved attacks today; it derives from carried weapons after this brief.
  - Shot origin: the fire proposal sets `WeaponAim.origin` from `perception.enemy_eye` (`compute.rs`), and `AiWeaponControllers::advance_actor` launches at it. `resolve_muzzle_launch_origin` (`crates/sim/src/weapon/mod.rs`) is private to sim, which the AI crate depends on. `HitZoneStore::posed_socket_world` samples a body socket's pose on demand; it does not compose an attachment's own socket. Pose availability at fire time is unverified (`research.md` §Not verified).
  - Per-set check: the reachability pass in stance lowering (`behavior_lowering/`).
  - Spawn: `attach_descriptor_components` (composition, placement key read from `MapEntity.key_values`, as `always_active` and `enabled_on_spawn` are). A placement warning can name its entity by `MapEntity::diagnostic_origin()`; the trap precedent names the `EntityId`.
  - Spawner: the per-copy loop in `spawn_resolved_spawners`. Its synthetic `MapEntity` has no KVP bag; read the spawner's own key with `registry.get_map_kvp(spawner_id, "equipment")`, beside the existing key reads in `entity_spawner.rs`. Spawner archetypes resolve at install (`crates/postretro/src/startup/lifecycle_spawners.rs`). `next_spawn_count` is claimed after `attach_descriptor_components`, and only when a Brain exists, so a draw before attach needs the claim moved ahead of it.
  - FGD: `entity_spawner` uses `base(EntitySpawn)`, not `AiEnemy`, so its `equipment` key is added there separately.
  - Draw seed: `behavior_draw_seed` is pure (FNV of name and salt), so a spawn-time draw can use `behavior_draw_unit(seed, stream, 0)` without a stored counter. A new `BehaviorDrawStream` variant grows the serde'd `counters` array. Placement salt: `placement_draw_salt` (`Placement(map_ordinal)`); spawned copies: `BehaviorDrawSalt::Spawned`.
  - Layers: `DefenseLayersComponent::from_descriptor` / `refreshed`, called from `plan_defense_layers_refresh` (`refresh_plan.rs`). `refreshed` clamps `value` to `capacity` before any reapply, so the overrides reapply before that clamp or the old value is carried across it. No per-instance override storage exists. Expression capacities re-evaluate every tick (`DefenseParameterSource::evaluate`) and would overwrite an override. Base expression parameters (`DefenseParameters`) stay keyed by canonical name.
  - Attachments: compose into `MeshComponent::with_attachments`, rebind via `resolve_mesh_entity_bindings_for_entities`. The E21 writer, `update_weapon_mount_attachments` (`crates/netcode/src/weapon_mounts.rs`), composes base plus one active entry for pawns only; composing several carried weapons for NPCs is new code. Model upload: `weapon_presentation_models` already sweeps every descriptor's weapon mounts on host and client. Set gear needs a sweep over every set an archetype lists; `distinct_mesh_models` / `resolved_spawner_mesh_models` read only base models and attachments.
  - Entry shape: `WeaponMountDescriptor` and `validate_weapon_mounts` (`crates/entities/src/data_descriptors/types/weapon_mounts.rs`). `attachments` is a plain map that `validate_weapon_mounts` rejects when empty, and `WEAPON_MOUNT_KEYS` / `check_weapon_mount_key` reject any other key, so `muzzle` joins the keys and `attachments` becomes optional.
  - Missing-entry warning: `check_weapon_mount_tables` already warns for an NPC table entry naming no weapon; set weapons without an entry join it.
  - Hot reload: `defer_visual_asset_descriptor_refreshes` covers `mesh`, so `mesh.weaponMounts` edits defer already. It does not cover an `equipment` block: set gear model paths need new deferral code and retention for a removed descriptor.
  - Weapon handles: `inventory.loadout` and `mesh.weaponMounts` already take weapon `defineEntity` values; `lowerWeaponMountReferences` (`sdk/lib/data_script.ts`, `.luau`) is the closest lowering precedent. `AttackParams.weapon` is `Option<String>`, and the SDK passes it through (`sdk/lib/data_script/attacks.ts`, `.luau`). `AttackParams` in `sdk/types/postretro.d.ts` and its Luau twin change shape. Every `content/` string call site is in `content/dev/scripts/limitator.ts`.
  - Wire: `RawEntityRecord`, `active_weapon_archetype` and `validate_active_weapon_metadata` (`crates/net/src/wire.rs`). That validator gates on the movement component, not the class, and the net crate is registry-blind (class prefixes live in `crates/netcode/src/descriptor_class.rs`). So the `descriptor:`-only rule needs a netcode-side rejection or a prefix check in `wire.rs`.
  - Client: remote NPCs materialize via `materialize_armed_remote_enemy` → `materialize_net_descriptor_presentation`. No client-side NPC attachment update exists; `update_weapon_mount_attachments` is the live-update precedent.
  - Guard read: `carries(weapon)` follows the prefix-namespaced path of `@attackTurnFree.` (`ATTACK_TURN_FREE_INPUT_PREFIX`, `BrainInputRef::AttackTurnFree`, `intern_attack_turn_read`), with the load check modeled on `check_guard_attack_turn_reads` (`crates/scripting-core/src/runtime/types.rs`). It does not go through `BRAIN_INPUTS`.
  - Group step: `NpcGroup` steps live in `sdk/lib/data_script/commands.ts` / `.luau`; Rust side `BoundTriggerCommand` (`crates/sim/src/trigger_commands.rs`). `applyStatus` is a partial precedent: it warns and skips per subject, but takes `HandleRef::Status`, and an equipment-set handle kind is new in `HandleRef`. The queued host-only steps (Trap, AttackPool) are the closer precedent for a mutating step that rebinds the mesh.
- Shape chosen: carried weapons, a composed mount list and layer capacities as per-instance state; sets as rig-independent authoring bundles; weapon mounts per archetype; an engine row skip.
- Rivals:
  - Set identity as the state. Rejected: later single-weapon changes would need an authored set for every combination, and graphs would branch on set names.
  - Per-set graph lowering with graph swap. Rejected: graph identity becomes the state, which forecloses single-weapon changes further. Its reachability check is borrowed.
  - Per-instance archetype selection: a placement or spawner draws from a weighted list of factory-built archetypes (the `rifleEnemy` precedent). Rejected: runtime equip still needs carried state, which leaves two mechanisms where this brief has one.
  - Weapon mounts inside sets. Rejected: mounts are per rig, so sets would multiply per rig, a mixed-rig `equip` would fail, and a dropped weapon would have no mount rule.
  - A separate NPC mount table. Rejected: players and NPCs would declare the same mount two ways, against the one-weapon-vocabulary rule.
  - Slot attacks, where one row fires whatever is carried. Rejected: weapons differ in reach and cadence, so behavior differs by weapon.
- First slice: carried-weapon filtering plus the row skip, on two placements in single player, before wire or equip work. It falsifies the riskiest assumption: that a skipped attack row leaves turns, tokens and clip moments clean.
- Files past ~800 lines this extends: `data_archetype.rs`, `net_descriptor.rs`, `wire.rs`, `graph_eval.rs`, `spawner.rs`, `replication.rs`, `brain_programs.rs`, `compute.rs`. Split a cohesive block out first where this adds one, in its own behavior-preserving commit.
- Derivation and verified facts: `research.md`.

## Open questions
- The tick on which an equip cancels a running burst relative to the fire proposal. — **delegated**
- What "a row to an unavailable attack" means when the row's target activity is compound: `RowSelection::skips` sees only the row's direct target. Recommended: the target activity and its initial-descent chain. — **delegated**
- A carried weapon with no `muzzle` socket to launch from: its mounted model has none, its entry's `muzzle` names a socket the body model lacks, it has no mount entry, or its archetype has no table. Recommended: launch from the eye; warn once at load where the archetype declares a table, and stay silent where it does not. — **delegated**
- A `muzzle` on a player pawn's entry: pawns launch from placement plus `muzzleOffset`. Recommended: it validates and is ignored, as an NPC's table was before this brief. — **delegated**
