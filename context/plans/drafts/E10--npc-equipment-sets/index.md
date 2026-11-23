# E10--npc-equipment-sets

Brief · resumable · draft, owner rulings of 2026-11-23 applied; owner API review pending · Epic 10 · reads: `context/lib/entity_model.md` §2 (Weapon vocabulary, Weapon mounts, Defense layers and damage types), §4, §7c · `context/lib/scripting.md` §1 (Authoring API style), §2, §11, §12 · `context/lib/networking.md` §Weapon placement is content, §Version gates, engine-randomness posture · `context/lib/resource_management.md` §7 (Sockets) · builds after `E10--npc-weapon-mounts`, then `E16--persistent-firing-modes` (drafts) · depends on `E21--character-weapon-mounts` (done) · read at e57fcf5ea

## Problem
Owner-requested capability (2026-11-13 roadmap item "Weapon-gated enemy attacks"; confirmed 2026-11-22): a modder or level designer equips individual NPCs of one archetype differently, and equipment can change during play. One grunt carries a rifle, the next a plasma rifle, armor and a visor. Today an enemy's weapons are fixed in its archetype's attack rows and its armor in its `health` block, and after `E10--npc-weapon-mounts` it mounts every weapon its attacks name. No placement key, spawner key or script step varies any of them, so each combination needs its own archetype and map classname, a spawner releases only one, and nothing can change what an NPC carries at runtime. When done:
- Each NPC instance carries its own weapons, wears its own gear and has its own defense layer capacities.
- An attack is available only while its weapon is carried, and only carried weapons and worn gear show on the NPC's rig.
- An archetype declares named equipment sets as authoring bundles, and a placement, spawner or script equips one.
- Co-op clients see what each NPC actually carries and wears.

## Decisions
- **Carried equipment is the runtime state; a set is how it is chosen.** An instance's state is its carried weapons, its worn gear items and its layer stack; its mounted list composes from them. The set it was equipped from does not survive spawn or `equip`. The gate, the guard and the wire read carried equipment only. So later single-item give, take, drop and pickup, of weapons or gear, are additive for the gate, guard and wire, though not for the per-set reachability check (below). "Carried" is named apart from held ownership (`entity_model.md` §3), held enemies (§7c) and the player's carried loadout across level changes (`CarriedLoadoutWire`).
- **Weapons are referenced by identity.** Every weapon reference this brief adds or touches takes the weapon archetype's `defineEntity` value, as `inventory.loadout` and `mesh.mounts` do: set weapons, the guard, and `defineAttack`'s `weapon`. The string form of `defineAttack`'s `weapon` is retired, and `content/` call sites migrate in this brief (pre-stable, `index.md`).
- **Gear is a first-class carried item.** `defineGear` returns a handle for a non-weapon worn prop and names no model: each character's `mesh.mounts` maps it to props baked for that rig, as for a weapon (`drafts/E10--npc-weapon-mounts/`), so gear stays rig-independent. The owner chose it as the shape that serves PostRetro longest: a set's gear is a default loadout, which later gear give and take override piece by piece, as for weapons.
- **Equipment set is a descriptor-owned authoring bundle.** `defineEquipment` returns a handle. It names weapons, gear items and capacity overrides for the archetype's defense layers. Sets are rig-independent and shareable across archetypes. An archetype lists its sets; the list needs no `defineMod` registration. A placement selects a declared set and never tunes one (`entity_model.md` §4).
- **Set and gear names are binding-name sugar.** Each is named by its binding, as damage types and defense layers are (`scripting.md` §2), in the forms the Boundary inventory pins. A set name diverges from §2's ghost-style rule, "The name is an explicit field, not binding-name sugar, because map files reference it as a string": the owner accepts it, as a damage type already reaches the map by its binding name through `crush_damage_type` (cost: `research.md` §Set names). No map names gear, so gear diverges from nothing.
- **Mounts compose over carried items, and gear wins its socket.** `E10--npc-weapon-mounts` composes an NPC's mounts and its shot origin from its carried weapons through `mesh.mounts`. Here:
  - A set's weapons are carried and its gear worn. An item neither carried nor worn does not mount.
  - The mounted list composes base `mesh.attachments`, then carried weapons' mounts, then worn gear's mounts, later winning per socket. The owner's rule: a gear set is a default loadout, and it overrides as needed.
  - Gear that displaces the model carrying a weapon's muzzle leaves that weapon no drawn muzzle: a missing muzzle under `E10--npc-weapon-mounts`, so its shots launch from the eye. Load warns where a set's own gear covers a socket its own weapon mounts (Load rules).
  - On an archetype with `equipment`, that brief's one-model-per-socket rule runs per set over the set's weapons instead of over every attack weapon (Load rules).
- **Equipment is not Inventory.** No held ownership, switching lane, reserve ammo or pickup. Weapon state is host-only, per carried weapon, and never a registry entity. `done/E16--enemy-ranged-projectile/`'s rejection of `Inventory` for enemies still holds (`research.md` §Prior commitments), and the one-weapon-vocabulary rule stands.
- **Layers: capacity overrides on declared layers.** A set's `layers` override the literal capacity of layers the archetype's `health` block declares. Stack shape, drain order, recharge and resist stay archetype-owned, so the stack never changes shape at runtime, keeping `grantLayer`'s existing-layers-only rule (`done/E16--damage-types-and-defense-layers/`). A capacity-0 layer is present and skipped by every hit (`scripting.md` §11), so an archetype declares armor at 0 and a set raises it. Health max, resist, hitbox, model, animations and behavior stay archetype-owned.
- **Gated attacks.** An attack that names a weapon is available only while the instance carries that weapon and AI can fire it: a projectile weapon that resolves. A row to an unavailable attack is skipped, as a failed guard is: it holds no turn, requests no token, plays no release and never whiffs, and the next row is evaluated. Contact attacks name no weapon and are never gated. An archetype without `equipment` carries every attack weapon, as under `E10--npc-weapon-mounts`, and the fireable rule holds there too: an attack whose weapon is hitscan or does not resolve is unavailable and its rows are skipped, replacing today's turn and whiff, which also set no cooldown. Graph evaluation stays host-only (§7c).
  - The gate applies to the active path too: an activity whose attack becomes unavailable reads as left on the next decide, through its first enabled row, else the graph's initial activity, and its attack turn ends, whether a mark, a hold or a release holds it.
- **Per-set reachability at load.** The stances brief rejects a reached rest left with no way out (`done/E10--enemy-stances-and-telegraphs/`). Lowering runs that check once per declared set, with that set's weapons carried. A failure rejects load, naming the set and the rest. The same pass rejects a set that leaves the attack of the graph's initial descent (the root's initial activity and each nested initial) unavailable, naming the set and the activity: the active-path fallback ends at the initial activity, so it must stay enterable. Give and take will reach carried states no set declares, so this check needs a runtime answer when they arrive.
- **Guard read.** `carries(weapon)` is true while the instance carries that weapon. On an archetype without `equipment`, it reads true for every weapon an attack names. Load rejects a weapon that none of the archetype's sets lists, or, without `equipment`, that no attack names.
- **Selection.** The `equipment` key on a placed enemy names one set. The same key on `entity_spawner` names one set for every copy.
  - With the key absent, on either, each instance draws by weight (default 1) from its per-instance behavior-draw seed. A weight-0 set is never drawn; only a key or `equip` gives it. The draw is host-only, at spawn, consequences-only, and independent of spawn order wherever the behavior draws are.
  - An unknown name warns, naming the key's entity, the archetype and the name, then draws.
  - A key on an entity whose archetype declares no `equipment` warns once, naming the entity, the archetype and the name, and the instance carries every attack weapon.
- **Runtime equip.** `npcs(...).equip(set)` is a host-only group step that replaces carried equipment with the set's.
  - Weapons: one carried before and after keeps its live state. One not in the set is dropped with its state, and a drawn or running attack on it is cancelled. One newly added arrives fresh.
  - Layers: a layer whose capacity the equip changes arrives full at its new capacity. A layer whose capacity is unchanged keeps its value.
  - Gear: the subject wears the set's gear items; gear the set lacks comes off.
  - The mounted list recomposes and rebinds.
  - A subject whose archetype lacks the set warns and is skipped; the other subjects still switch.
  - A subject at zero health is skipped silently, its state unchanged: re-arming a corpse is meaningless, and a squad-wide equip would otherwise warn for each fallen member. A held subject is equipped like any other, so on release it carries what its squad carries.
  - An equip applies at the start of the next tick, before every impact phase (mover crush included), so every hit from then on drains the post-equip stack, the same under every frame split. A hit on the reaction's own tick drains the pre-equip stack.
  - Two equips on one tick both apply at the next tick's start, in reaction order.
  - Every model an archetype's `mesh.mounts` lists loads with the level on every peer, so an equip never waits on a load.
- **Replication carries carried weapon and worn gear identities.** An equipment-bearing NPC's snapshot record gains its carried weapons' canonical names and its worn gear names (Wire format).
  - Each client composes the mounts from its own descriptors with `E10--npc-weapon-mounts`'s composer over the listed weapons and gear, as it composes a pawn's from `active_weapon_archetype`. With the field absent, it composes as for an NPC without equipment.
  - A listed item the client's table lacks mounts nothing under the missing-entry rule; a mount model the client has not loaded is skipped with one warning (E21).
  - Layers and the gate have no client consumer and stay host-only. Fire sounds and projectiles already carry their own identity.
  - The compatibility digest stays as it is: descriptors resolve locally by name (`entity_model.md` §7d precedent).
- **Hot reload never re-equips.** Carried weapons, worn gear, the mounted list and the layer stack stay as worn, as a live player loadout does (`done/E16--wieldable-switching-inventory/`). Weapon descriptor edits reach carried weapons as today. Set edits reach the next spawn or `equip`. Mount edits, gear's included, apply at the next level load, as every `mesh` edit does. A base-descriptor reload rebuilds the layer stack under the existing match-by-name rule and applies the instance's capacity overrides before kept values clamp, so a value clamps to the override capacity. A carried weapon the reload removes leaves its attacks unavailable.
- **A reload that adds or removes `equipment` applies the new rule at once.** It is the one deliberate exception to never re-equipping. Removing `equipment` makes each live instance carry every attack weapon, its gear dropped and its set's layer-capacity overrides dropped: layers return to the archetype's base capacities, values clamping to them, as if spawned without sets. Adding it equips each live instance as at spawn: by its key, else by draw.
- **Load rules (both runtimes, twin behavior).** Each of these rejects load:
  - an empty list, a duplicated set, or a weight that is not finite and ≥ 0;
  - a list whose weights are all 0;
  - an entry key other than `set` and `weight`;
  - a capacity override naming a layer the archetype does not declare, a layer whose base capacity is an expression, or a capacity that is not a finite literal ≥ 0 (the rejection names the set and the layer);
  - two weapons of one listed set whose mount entries put different models on one socket. The rejection names the set, the archetype and the socket;
  - a `mesh.mounts` entry for a gear item that names `muzzle`: gear fires no shot, so the entry would draw nothing. The rejection names the character and the gear item;
  - a gear name equal to a weapon's canonical name, so a mount entry's `item` resolves to one kind. The rejection names both;
  - a set that lists one weapon or gear item twice, overrides one layer twice, or carries a key other than `weapons`, `gear` and `layers`. The rejection names the set and the item or key.

  These warn and load:
  - `equipment` on a descriptor without `behavior`; the block drops.
  - A set weapon that is not a projectile weapon, once per archetype, set and weapon: AI cannot fire it, which is unsupported, not invalid. Its attacks stay unavailable. An archetype without `equipment` has no set to warn on; its hitscan-weapon attack keeps the runtime warn-once.
  - On an archetype that declares `equipment`, a mount entry for an item none of its sets lists, once at load, naming the archetype and the item; it never mounts. An archetype without `equipment` stays silent (Designers over structure, `index.md` §2).
  - A set weapon or gear item without a mount entry, under `E10--npc-weapon-mounts`'s missing-entry rule.
  - A set whose gear's mount covers a socket one of its own weapons' mounts uses, once at load, naming the archetype, the set, the gear item, the weapon and the socket. The gear wins the socket.
- **Non-goals.**
  - Single-item give, take, drop on death, NPC pickup and scavenging, of weapons or gear: later Epic 10 work, additive on carried state; each item brings its own mount.
  - Gear that grants layers, stats or behavior: a set's `layers` carry armor, and gear is worn identity. Widening gear later breaks no content.
  - A guard on worn gear: `carries` takes a weapon. A gear read is additive later.
  - A set restricting a modal weapon's modes: a set carries a weapon with every mode it declares, and attacks choose modes (`drafts/E16--persistent-firing-modes/`).
  - Converting other string weapon references to handles: the one left is the projectile trap's `weapon` map key, which stays a string because a map resolves it by name (`scripting.md` §1, Handles over strings).
  - Hitscan enemy weapons: AI fire stays projectile-only. An NPC hitscan weapon is valid content and may come in a later brief; dev content stays projectile-only.
  - Two of one weapon (dual wield): weapon state is keyed by weapon.
  - Per-set layer kinds, recharge, resist, health max, model or animations. Widening overrides later breaks no content.
  - Generating FGD `choices` from descriptors: the FGD stays hand-written, and the key is a string.
  - Player use: player pawns keep `inventory.loadout`.
  - Replicating NPC weapon resource state: no client consumer, since enemies have no HUD and weapon cues carry resolved sounds.
- **Docs amended at promotion:**
  - `entity_model.md`: Components, Weapon vocabulary (identity references), Weapon mounts (a set's weapons are carried and its gear worn; worn gear composes last and wins its socket; gear that displaces a muzzle leaves the eye shot; the one-model rule per set; gear entries never name `muzzle`; weapon and gear names never collide; an unused entry warns under `equipment`), Defense layers (set capacity overrides), §4 equipment selection, §7c gate (the active path included) and per-set check.
  - `scripting.md`: §2 (equipment sets take binding-name sugar, an exception to the ghost-style explicit-name rule beside damage types; gear declarations follow the damage-type rule), §11, §12 group steps.
  - `networking.md`: an NPC's carried weapons and worn gear on its record, composed on each client from identity; §Version gates history; the randomness posture (set draws ride the enemy behavior-draw exception and run at spawn).
  - `build_pipeline.md`: §Custom FGD.
  - Human-facing: `docs/scripting-reference.md`, `docs/level_design.md`, `docs/weapon-mounts.md` (gear entries in `mesh.mounts`), `sdk/TrenchBroom/postretro.fgd`.

### Scripting surface
Owner reviews this surface personally before promotion.
```ts
import { brain, carries, defineAttack, defineEntity, defineEquipment, defineGear, defineReaction, npcs } from "postretro";
import { armor } from "./defenses";
import { enemyRifleEntity as rifle } from "./enemy-rifle";
import { referencePlasmaBoltEntity as plasmaRifle } from "./reference-projectiles";

// A worn item, named by binding. It names no model: each rig's `mesh.mounts` does.
// Luau passes the name: defineGear("visor").
export const visor = defineGear();

// Rig-independent bundles of weapons, gear and layer capacities, named by binding:
// the map key `equipment` reads "rifleKit" / "plasmaKit". Luau passes the name: defineEquipment("rifleKit", { … }).
export const rifleKit = defineEquipment({ weapons: [rifle] });
export const plasmaKit = defineEquipment({
  weapons: [plasmaRifle],
  gear: [visor], // a default loadout
  layers: [{ layer: armor, capacity: 40 }], // overrides a declared layer's capacity
});
export const disarmedKit = defineEquipment({ weapons: [] });

const shoot = defineAttack({ weapon: rifle /* positioning, draws, release… */ });
const bolt = defineAttack({ weapon: plasmaRifle /* … */ });

export const grunt = defineEntity({
  canonicalName: "grunt",
  components: {
    // faction as for any NPC
    mesh: {
      model: "models/grunt/model.gltf",
      // Table owned by E10--npc-weapon-mounts. Mounted only while carried or worn; gear wins its socket.
      // Different models on `hand_r` load: no set carries both weapons.
      mounts: [
        { item: rifle, attachments: { hand_r: "models/ar_4/grunt.gltf" } },
        { item: plasmaRifle, attachments: { hand_r: "models/cyberpunk_weapons/rpg/grunt.gltf" } },
        { item: visor, attachments: { head: "models/visor/grunt.gltf" } }, // baked for this rig
      ],
      // animations…
    },
    health: { max: 100, layers: [{ layer: armor, capacity: 0 }] }, // present, skipped until a set raises it
    equipment: [
      { set: rifleKit, weight: 3 },
      { set: plasmaKit }, // weight defaults to 1
      { set: disarmedKit, weight: 0 }, // never drawn; a key or equip gives it
    ],
    behavior: {
      initial: "idle",
      activities: {
        idle: { animation: "idle", motion: "hold" },
        kite: { animation: "walk", motion: "strafe" },
        shoot: { animation: "shoot", motion: "chaseTarget", action: { attack: shoot } }, // skipped unless carried
        bolt: { animation: "shoot", motion: "hold", action: { attack: bolt } },
      },
      transitions: {
        "*": [{ to: "idle", when: brain.hasTarget.not() }],
        idle: [
          // `carries` gates a row no attack gate covers: a plasma carrier strafes before it fires up close.
          { to: "kite", when: carries(plasmaRifle).and(brain.hasTarget).and(brain.targetDistance.lt(6)) },
          { to: "shoot", when: brain.hasTarget },
          { to: "bolt", when: brain.hasTarget },
        ],
        kite: [{ to: "bolt", when: brain.timeInActivityMs.gt(1500) }],
      },
    },
  },
});

export const rearm = defineReaction("squad.rearm", npcs({ tag: "squad_b" }).equip(plasmaKit));
```

### Boundary inventory
Both runtimes ship.

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| set declaration | descriptor equipment table | `"equipment"` (list of `{set, weight}`) | `defineEquipment`, `components.equipment` | `defineEquipment`, `components.equipment` | n/a |
| set name | binding name | string | binding name, inserted at build; may be passed explicitly | explicit first argument, as for `defineDefenseLayer` | `equipment` on `AiEnemy` and `entity_spawner` |
| set gear | gear items a set lists | `"gear"` in a set: list of gear names | `gear` | `gear` | n/a |
| gear item | gear name | string | `defineGear()`, binding name inserted at build; may be passed explicitly | `defineGear("<name>")`, explicit first argument, as for `defineDefenseLayer` | n/a |
| weapon reference | canonical name | canonical name string | weapon `defineEntity` value | weapon `defineEntity` value | n/a |
| mount item | weapon canonical name or gear name; the two never collide | name string in a `mounts` entry's `"item"` | weapon `defineEntity` value or gear handle | weapon `defineEntity` value or gear handle | n/a |
| mounts | defined by `E10--npc-weapon-mounts` | as defined there | as defined there | as defined there | n/a |
| guard read | brain scope, name-keyed leaf | `@carries.<weapon>` | `carries(weapon)` | `carries(weapon)` | n/a |
| group step | typed command | n/a | `npcs(…).equip(set)` | `npcs(…):equip(set)` | n/a |
| NPC carried equipment | snapshot record, optional: carried weapons and worn gear | see Wire format | n/a | n/a | n/a |

### Wire format
NPC carried equipment on the snapshot entity record, bitcode like the record's other fields, mirroring `active_weapon_archetype`'s presence-flag shape:
- A presence flag. Absent: the client composes as for an NPC without equipment. A false flag with a non-empty weapon or gear list is malformed, as for `active_weapon_archetype`.
- Present: the carried weapons' canonical names, then the worn gear names, each list sorted with no duplicates. An out-of-order or duplicated name in either list is malformed. An empty list carries none.
- Valid only on records whose class is `descriptor:`. The net crate is registry-blind and checks only the flag's consistency, so netcode rejects a record of any other class carrying it. `validate_active_weapon_metadata`, which rejects misplaced metadata, is the precedent.
- Bumps `WIRE_VERSION` (43 at read) and `SNAPSHOT_VERSION` (27 at read).

## Acceptance
### Automated
**Per-instance equipment**
- [ ] Two placements of one archetype, keyed to different sets: each fires only the weapons it carries, has its set's layer capacities, and mounts the composed list of base attachments, then carried weapons' mounts, then worn gear's mounts.
- [ ] One set listed by two archetypes on different rigs mounts each archetype's own bake for the same weapon, and each archetype's own model for the same gear item.
- [ ] A set without `layers` keeps the base capacities. A set that raises a capacity-0 layer makes it drain before health. An instance whose set leaves it at 0 takes every hit on health.
- [ ] An archetype without `equipment` fires and mounts every attack weapon and loads as under `E10--npc-weapon-mounts`.
- [ ] An equipment-bearing NPC holds no `Inventory`, and spawning it or equipping it adds no registry entity for a carried weapon or gear item.

**Gear wins its socket**
- [ ] Gear whose mount names a socket that a base attachment and a carried weapon's mount both use draws the gear there, whatever order its set lists its weapons and gear in.
- [ ] A set whose gear's mount covers a socket its own weapon's mount uses loads in both runtimes and warns once at load, naming the archetype, the set, the gear item, the weapon and the socket. Equipped, the gear draws on that socket. When it displaces the model carrying the weapon's `muzzle`, that weapon's shots launch from the eye; when it displaces another of that weapon's models, shots still leave the muzzle. The same gear on a socket where only another set's weapon mounts loads without that warning.

**Gate, both sides**
- [ ] With a weapon carried, its attack fires and its mount shows. With it not carried, the row is never taken, whether it enters a leaf whose action fires the attack, strikes the attack from its stance's rest, or is a `"*"` row: no turn held, no token requested, no release animation, no whiff, and the next row runs on the same decide tick. Its mount is absent.
- [ ] An offense selector whose first passing row fires an attack on a weapon not carried passes over that row as a failed guard, and its next passing row fires a carried weapon's attack on the same tick; with no carried row passing, the activity fires nothing and requests no turn.
- [ ] A contact attack fires under every set, including an empty-weapon set.
- [ ] On an archetype without `equipment`, an attack whose weapon is hitscan or does not resolve is unavailable: its rows are skipped, with no turn, no whiff and the next row run on the same decide tick, while a projectile attack beside it still fires.
- [ ] `carries(weapon)` reads true only while that weapon is carried. Naming a weapon no set lists rejects load.
- [ ] On an archetype without `equipment`, `carries(weapon)` reads true for every weapon an attack names, and naming a weapon no attack names rejects load.
- [ ] An NPC in its `shoot` activity when an equip drops the rifle leaves `shoot` on the next decide, through its first enabled row, else the graph's initial activity, and its attack turn frees, whether a mark, a hold or a release held it (pin E-1). Under an equip that keeps the rifle, it continues in `shoot` with its turn.

**Per-set reachability**
- [ ] A set under which a reached rest has no way out rejects load, naming the set and the rest. The same graph with that set removed loads.
- [ ] A set that leaves the attack of the graph's initial descent unavailable, at the root or at a nested initial, rejects load, naming the set and the activity. The same graph with that set carrying the weapon loads.

**Selection**
- [ ] With no key, the same seed gives every placement and spawned copy the same set across two loads, and spawn order does not change it.
- [ ] Over 1,000 drawn copies at weights 3:1:0, the first set's share is within ±5% of 75%, and the weight-0 set is never drawn. A single drawable set always yields that set.
- [ ] A placement key equal to a set's TypeScript binding name, or to its Luau explicit name, equips that set.
- [ ] A key naming a weight-0 set equips it. A spawner key naming a set gives every copy that set. An unknown name on a placement or spawner warns, naming it, then draws.
- [ ] A spawner whose `equipment` key names an unknown set warns once per level load, not once per copy, and every copy draws.
- [ ] An `equipment` key on a placement or spawner whose archetype declares no `equipment` warns once, naming the entity, the archetype and the name, and the instance carries every attack weapon.
- [ ] After a same-map restart, every placement and spawned copy carries its keyed or drawn set again, whatever an `equip` gave it before (pin E-6).

**Runtime equip**
- [ ] An equip during a dropped weapon's burst cancels it, and no further shot from that weapon fires; its mount leaves on the tick the equip applies.
- [ ] After an equip, a newly carried weapon's shots launch from its own mount's muzzle.
- [ ] A weapon kept across the equip keeps its ammo or heat, and a burst in flight on it fires its remaining shots (pin E-7). A layer whose capacity the equip changes arrives full at the new capacity; one with an unchanged capacity keeps its value.
- [ ] Equipping a set that matches what is carried and worn changes no weapon state or layer value, and a burst in flight keeps firing.
- [ ] An activity whose offense selector lists a dropped weapon's attack beside a carried one stays active across the equip and fires the carried attack. Only an activity whose own action, mark, hold or release names the dropped weapon's attack reads as left.
- [ ] An NPC alive when a reaction equips it and killed later on that tick is skipped without a warning when the equip applies. A queued equip whose subject despawns first, or that a same-map restart or level unload overtakes, applies to nothing and warns nothing (pin E-8).
- [ ] An NPC whose AI tier skips ticks, equipped while it holds the dropped weapon's attack turn, fires nothing from that weapon while skipped and frees the turn on its next decide (pin E-9).
- [ ] A reaction that spawns copies and then equips their tag leaves each copy on its spawn set for the spawn tick, that tick's snapshot included, and on the equipped set from the next tick's start (pin E-10).
- [ ] One `equip` on a squad of two archetypes on different rigs gives each its own mount for the same weapon and its own model for the same gear item, and gear the new set lacks comes off. A subject lacking the set warns and is skipped while the others switch.
- [ ] Two equips on one tick both apply at the next tick's start, in reaction order, and leave the later one's weapons carried and its gear worn; a weapon or layer the first equip dropped or changed and the second restores arrives fresh or full, as two equips on separate ticks leave it (pin E-5).
- [ ] An `equip` whose subjects include an NPC at zero health skips it without a warning, leaving its mounts, layers and weapon state as they were, and the other subjects switch. A held NPC among them is equipped like the rest and, once released, fires and mounts the new set (pin E-3).
- [ ] A hit on the tick a reaction equips the NPC drains the whole pre-equip stack. Every hit from the next tick on, a mover crush hit included, drains the whole post-equip stack, never a mix, the same under every frame split (pin E-2).
- [ ] An equip to a set whose weapon and gear models no NPC showed at level load draws them on the tick the equip applies, on the host and on a connected client, with no missing-model warning.

**Reload and load rules**
- [ ] Hot reload leaves carried weapons, worn gear, mounts and layer stacks as worn. A weapon descriptor edit reaches a carried weapon. A set edit reaches the next spawn and the next equip, not live instances; a mount model edit, gear's included, applies at the next level load. A base-descriptor reload that edits a layer's recharge reaches equipped instances; their capacity overrides stay applied, and each layer keeps its value: armor at 30 of a set's 40, over a base capacity of 0, reads 30 of 40 after the reload.
- [ ] A hot reload that removes `equipment` from an archetype makes each live instance carry every attack weapon at once, its gear off and its layers back at the archetype's base capacities, values clamped to them: armor at 30 of a set's 40, over a base capacity of 0, reads 0 of 0. One that adds `equipment` equips each live placement by its own `equipment` key and each live spawned copy by its spawner's key, else by draw.
- [ ] Each rejecting load rule in Decisions rejects in both runtimes: empty list, duplicate set, bad weight, all-zero weights, an unknown entry key (an `attachments` key on an equipment entry among them), override of an undeclared layer, override of an expression-capacity layer, bad override capacity, a gear mount entry naming `muzzle`, a gear name equal to a weapon's canonical name, a set listing one weapon or gear item twice, a set overriding one layer twice, a set key other than `weapons`, `gear` and `layers`, the last three naming the set and the item or key. `equipment` without `behavior` warns and loads.
- [ ] A set listing a hitscan weapon loads in both runtimes and warns once per archetype, set and weapon that AI cannot fire it. An NPC carrying it never fires its attacks: rows to them are skipped as an uncarried weapon's are, and the NPC fires its other carried weapons.
- [ ] A set whose weapons' mounts put different models on one socket, on an archetype listing it, rejects load, naming the set, the archetype and the socket. The same set loads on an archetype whose mounts for those weapons use different sockets, or name the same model on the shared socket. Two weapons in different sets whose mounts put different models on one socket load.
- [ ] A gear mount naming a socket the rig lacks warns once per archetype, gear item and socket, and draws nothing there; the entry's other sockets mount.
- [ ] On an archetype with `equipment`, a mount entry for a weapon or gear item no set lists warns once at load, naming the archetype and the item, mounts nothing, and the archetype loads. The same entry on an archetype without `equipment` logs nothing.
- [ ] On an archetype with `mesh.mounts`, a set weapon or gear item without an entry warns once at load, naming the archetype and the item, and mounts nothing; the weapon's shots launch from the eye. A later spawn or equip carrying it does not warn again. An archetype without a table carries the same set silently.
- [ ] `defineAttack`, a set's `weapons` and `carries` each accept a weapon handle and reject a string, in both runtimes. Every `content/` attack resolves its weapon after migration.
- [ ] A hot reload that removes a carried weapon's descriptor leaves its attacks unavailable as an uncarried weapon's are: rows to them are skipped and the NPC fires its other carried weapons.

**Replication**
- [ ] A connected client mounts each equipment-bearing NPC from the host's carried weapon and worn gear lists through its own descriptors, at materialize and again after an equip; the snapshot stamped with the tick the equip applies carries the new lists, and a client that joins, or an NPC that re-enters a client's relevance, after the equip mounts the new lists (pin E-4). Empty lists mount no weapon and no gear. An NPC without equipment mounts as under `E10--npc-weapon-mounts`.
- [ ] Two archetypes on different rigs carrying the same weapon and wearing the same gear each show their own rig's models on a connected client.
- [ ] A listed weapon or gear item the client's table lacks mounts nothing, under `E10--npc-weapon-mounts`'s missing-entry rule; a mount model the client has not loaded is skipped with one warning.
- [ ] A pawn or projectile record carrying the field is rejected. A record whose presence flag is false and that carries weapons or gear is rejected, and so is one with an out-of-order or duplicated name in either list.

**Surface**
- [ ] The Scripting surface example runs as a `content/dev` fixture in both runtimes and lowers to the same IR.

**Capacity**
- [ ] Measure and report AI tick cost and snapshot bytes on a 256-enemy mixed-set stress fixture, against the same fixture with `equipment` removed, under `context/lib/testing_guide.md` §Resource bounds.
### Manual
- [ ] On a dev map, two placements of one class visibly carry different guns seated in the hand, one wearing its set's gear, and a trigger reaction swaps one's gun in hand.
- [ ] Two-client co-op at 150 ms RTT (`POSTRETRO_NET_LAG`): the client sees each NPC's host-carried weapon and worn gear, before and after an equip.

## Path
- Seams (symbols as read at e57fcf5ea, before `E10--npc-weapon-mounts` renames the table):
  - Attacks: `AttackParams.weapon` → `resolve_projectile_attacks` (already per entity, in `BrainEntityPrograms`). Filter it by carried weapons.
  - Row skip: `RowSelection::skips` in `graph_eval.rs`, used by `first_enabled_target`. It sees the row and its direct target activity (`envelope.activities.get(&row.to)`), not nested descendants. `TurnGate` sits beside it.
  - Cancellation: `arbitrate_attack_turns` cancels an execution whose turn ended, any execution on a reseat, and one whose attack no longer resolves or whose enemy holds no turn for it. A weapon that stops resolving is dropped in `AiWeaponControllers::sync` (`retain`, clearing `active`). `sync`'s early exit keys on graph `Arc::ptr_eq` and `descriptor_generation`; it must also key on carried equipment, or an `equip` never re-syncs.
  - Weapon state: `AiWeaponControllers::sync` derives it from resolved attacks today; it derives from carried weapons after this brief.
  - Per-set check: the reachability pass in stance lowering (`behavior_lowering/`).
  - Spawn: `attach_descriptor_components` (composition, placement key read from `MapEntity.key_values`, as `always_active` and `enabled_on_spawn` are). A placement warning can name its entity by `MapEntity::diagnostic_origin()`; the trap precedent names the `EntityId`.
  - Spawner: the per-copy loop in `spawn_resolved_spawners`. Its synthetic `MapEntity` carries only `enabled_on_spawn` in its KVP bag; read the spawner's own key with `registry.get_map_kvp(spawner_id, "equipment")`, beside the existing key reads in `entity_spawner.rs`. Spawner archetypes resolve at install (`crates/postretro/src/startup/lifecycle_spawners.rs`). `next_spawn_count` is claimed after `attach_descriptor_components`, and only when a Brain exists, so a draw before attach needs the claim moved ahead of it.
  - FGD: `entity_spawner` uses `base(EntitySpawn)`, not `AiEnemy`, so its `equipment` key is added there separately.
  - Draw seed: `behavior_draw_seed` is pure (FNV of name and salt), so a spawn-time draw can use `behavior_draw_unit(seed, stream, 0)` without a stored counter. A new `BehaviorDrawStream` variant grows the serde'd `counters` array. Placement salt: `placement_draw_salt` (`Placement(map_ordinal)`); spawned copies: `BehaviorDrawSalt::Spawned`; a spawn with no map ordinal: `BehaviorDrawSalt::Entity`, replay-stable only while spawn order is. The script compiler has no descriptor knowledge, so a key's set name is checked at load.
  - Layers: `DefenseLayersComponent::from_descriptor` / `refreshed`, called from `plan_defense_layers_refresh` (`refresh_plan.rs`). `refreshed` clamps `value` to `capacity` before any reapply, so the overrides reapply before that clamp or the old value is carried across it. No per-instance override storage exists. Expression capacities re-evaluate every tick (`DefenseParameterSource::evaluate`) and would overwrite an override. Base expression parameters (`DefenseParameters`) stay keyed by canonical name.
  - Attachments: `E10--npc-weapon-mounts` composes an NPC's mounts in `descriptor_mesh_component` from a carried list fixed per archetype; here the list is per instance, spans weapons and gear, and changes live, on host and client. Rebind via `resolve_mesh_entity_bindings_for_entities`; `update_weapon_mount_attachments` is the live-update precedent, and no client path updates an NPC's attachments after materialize. Model upload: `weapon_presentation_models` already sweeps every table model, so gear entries need no new sweep.
  - Gear entries: the mount entry, its validation and its SDK lowering (`WeaponMountDescriptor`, `validate_weapon_mounts`, `lowerWeaponMountReferences`) gain gear items once renamed; resolving an `item` must tell a weapon from a gear name.
  - Mount warnings: `check_weapon_mount_tables` (`crates/sim/src/scripting_systems/weapon_mounts.rs`); set weapons, set gear, unused entries and gear covering a weapon socket join it.
  - Hot reload: `defer_visual_asset_descriptor_refreshes` covers `mesh` whole, so table edits, gear's included, defer already. It does not cover `equipment`, so a reload that adds or removes the block reaches live instances through the refresh plan (`refresh_plan.rs`).
  - Equip timing: `apply_attack_pool_requests` runs after `ingest_ready_remote_hits` and `projectile_stage::advance` in `crates/sim/src/sim/tick.rs`, so an equip queued like the AttackPool step would land after that tick's hits. The equip drains at the top of the next tick instead, ahead of every impact phase. The tick that applies it also stamps the snapshot that carries the new lists.
  - Hitscan set weapons: `resolve_projectile_attacks` disables a hitscan weapon's attack with a runtime warn-once today, and no load check exists; the set warning is new at load. The row skip must read that availability on every archetype, not only under `equipment`, since a disabled attack today still takes a turn and whiffs.
  - Spawned copies and reload: a copy keeps no link to its spawner's key today (its synthetic `MapEntity` bag carries only `enabled_on_spawn`; the copy keeps a draw seed and schedule word). The executor stores the key or a link at spawn, or a reload that adds `equipment` cannot equip copies by their spawner's key.
  - Weapon handles: `lowerWeaponMountReferences` (`sdk/lib/data_script.ts`, `.luau`) is the closest lowering precedent. `AttackParams.weapon` is `Option<String>`, and the SDK passes it through (`sdk/lib/data_script/attacks.ts`, `.luau`). `AttackParams` in `sdk/types/postretro.d.ts` and its Luau twin change shape. Every `content/` string call site is in `content/dev/scripts/limitator.ts`.
  - Set and gear names: the script compiler inserts binding names for `defineDefenseLayer` and `defineAttackToken` (`crates/script-compiler/src/lib.rs`); `defineDefenseLayer`'s explicit-name overloads (`sdk/lib/data_script.ts`, `.luau`) are the Luau precedent. A handle there is `{ kind, name }`.
  - Wire: `RawEntityRecord`, `active_weapon_archetype` and `validate_active_weapon_metadata` (`crates/net/src/wire.rs`). That validator gates on the movement component, not the class; class prefixes live in `crates/netcode/src/descriptor_class.rs`.
  - Guard read: `carries(weapon)` follows the prefix-namespaced path of `@attackTurnFree.` (`ATTACK_TURN_FREE_INPUT_PREFIX`, `BrainInputRef::AttackTurnFree`, `intern_attack_turn_read`), with the load check modeled on `check_guard_attack_turn_reads` (`crates/scripting-core/src/runtime/types.rs`). It does not go through `BRAIN_INPUTS`.
  - Group step: `NpcGroup` steps live in `sdk/lib/data_script/commands.ts` / `.luau`; Rust side `BoundTriggerCommand` (`crates/sim/src/trigger_commands.rs`). `applyStatus` is a partial precedent: it warns and skips per subject, but takes `HandleRef::Status`, and an equipment-set handle kind is new in `HandleRef`. The queued host-only steps (Trap, AttackPool) are the closer precedent for a mutating step that rebinds the mesh.
- Build order: after `drafts/E16--persistent-firing-modes/`, the limitator and the rifle squads carry one modal rifle, so a set lists it once and its attacks choose modes. The example's weapons are `enemy_rifle` (`enemy-rifle.ts`) and `reference_plasma_bolt` (`reference-projectiles.ts`), both projectile weapons E16 leaves separate; the plasma rifle's refilling cell needs no reload, and its secondary is never driven. If E16 renames `enemy_rifle`, the fixture follows. The grunt, its bakes and its visor are fixtures this brief adds.
- Shape chosen: carried weapons, worn gear items, a composed mount list and layer capacities as per-instance state; sets as rig-independent bundles of weapons and gear; per-rig gear models in `mesh.mounts`; an engine row skip; carried weapon and worn gear identities on the wire.
- Rivals, the strongest two (all weighed: `research.md` §Rivals weighed):
  - Set identity as the state. Rejected: later single-weapon changes would need an authored set for every combination, and graphs would branch on set names.
  - Per-instance archetype selection: a placement or spawner draws from a weighted list of factory-built archetypes (the `rifleEnemy` precedent). Rejected: runtime equip still needs carried state, which leaves two mechanisms where this brief has one.
- First slice: carried-weapon filtering plus the row skip, on two placements in single player, before wire or equip work. It falsifies the riskiest assumption: that a skipped attack row leaves turns, tokens and clip moments clean.
- Files past ~800 lines this extends: `data_archetype.rs`, `net_descriptor.rs`, `wire.rs`, `graph_eval.rs`, `spawner.rs`, `replication.rs`, `brain_programs.rs`, `compute.rs`. Split a cohesive block out first where this adds one, in its own behavior-preserving commit.
- Derivation and verified facts: `research.md`; mount and shot-origin facts: `drafts/E10--npc-weapon-mounts/research.md`.

## Open questions
- The tick on which an equip cancels a running burst relative to the fire proposal. — **delegated**
- What "a row to an unavailable attack" means when the row's target activity is compound: `RowSelection::skips` sees only the row's direct target. Recommended: the target activity and its initial-descent chain. — **delegated**
- Two worn gear items of one set whose mounts put different models on one socket. Recommended: the later in name order wins, matching the wire's sorted gear list so every peer draws the same one, and load warns once, naming the archetype, the set and the socket. — **delegated**
- `drafts/descriptor-instance-overrides/`: a base weapon's `mesh.mounts` entry must count as serving the variants a set lists, for the unused-entry and missing-entry warnings and the per-set one-model rule, while `carries` keys on the variant itself. Whichever of the two briefs builds second reconciles it and reports it in the plan of record. — **delegated**
