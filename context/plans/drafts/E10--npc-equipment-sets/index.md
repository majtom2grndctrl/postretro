# E10--npc-equipment-sets

Brief · resumable · draft, not yet validated · Epic 10 · reads: `context/lib/entity_model.md` §2 (Weapon vocabulary, Defense layers and damage types), §7c · `context/lib/scripting.md` §1 (Authoring API style), §2, §11, §12 · `context/lib/networking.md` §Version gates, engine-randomness posture · `context/lib/resource_management.md` §7 (Sockets) · read at e761fb13

## Problem
Owner-requested capability (2026-11-13 roadmap item "Weapon-gated enemy attacks"; confirmed 2026-11-22): a modder or level designer equips individual NPCs of one archetype differently — one grunt carries a burst rifle and a visor, the next a marksman rifle and armor. Today an enemy's weapons are fixed in its archetype's attack rows, its armor in its `health` block, its visible gun in `mesh.attachments`. No placement key, spawner key or script step varies any of them, so each combination needs its own archetype and map classname, and a spawner releases only one. When done, an archetype declares named equipment sets. A placement or spawner picks one, or draws one by weight. An attack whose weapon the instance has not equipped is unavailable. A script can switch an NPC's set during play, and co-op clients see the weapon each NPC actually carries.

## Decisions
- **Equipment set is a descriptor-owned noun.** `defineEquipment` returns a handle named by its binding, as defense layers are (`scripting.md` §1, Handles over strings). It bundles weapons (canonical weapon names, as attack rows name them), attachments (socket → prop model) and defense layer entries. An archetype lists its sets; the list needs no `defineMod` registration. A placement selects a declared set and never tunes one, keeping `entity_model.md` §4.
- **Equipment is not Inventory.** No held ownership, switching lane, reserve ammo or pickup. Weapon state stays in the AI weapon controllers. This keeps `done/E16--enemy-ranged-projectile/`'s decision that enemies carry no `Inventory`, and the one-weapon-vocabulary rule: a set names the same `WeaponDescriptor`s a player loadout does.
- **Composition over the base descriptor.** Set attachments merge over `mesh.attachments` by socket, and the set wins. A set's `layers`, when present, replace the base stack; absent, the base stack stands (`scripting.md` §1: an explicit list replaces the inferred one). Health max, resist, hitbox, model, animations and behavior stay archetype-owned. Mounts are baked per rig (`resource_management.md` §7), so a set names its attachment explicitly and never borrows the weapon's `thirdPersonModel`.
- **Gated attacks.** An attack that names a weapon is available only while the instance's equipped set lists that weapon. A row to an unavailable attack is skipped, as a failed guard is: it holds no turn, requests no token, plays no release and never whiffs. The next row is evaluated. Contact attacks name no weapon and are never gated. An archetype without `equipment` treats every attack weapon as equipped, exactly as today. Graph evaluation stays host-only (§7c).
- **Guard read.** `equipped(set)` is true while the instance wears that set. Load rejects a set outside the archetype's list, naming both, as `attackTurnFree` rejects a foreign attack.
- **Selection.** The `equipment` key on a placed enemy names one set. The same key on `entity_spawner` names one set for every copy. With the key absent, each instance draws by weight (default 1) from its per-instance behavior-draw seed on a new stream. The draw is host-only, at spawn, consequences-only, and independent of spawn order (`networking.md` randomness posture; `entity_model.md` §7c draws). An unknown name warns, naming the key's entity, the archetype and the name, then draws. The compiler has no descriptor knowledge, so the name is checked at load, not at bake.
- **Runtime switch.** `npcs(...).equip(set)` is a host-only group step. Weapons in both sets keep their live state. A dropped weapon loses its state, and a drawn or running attack on it is cancelled. An added weapon arrives fresh. Layers follow the hot-reload rule by name: a kept layer keeps its value, clamped; an added layer arrives full; a removed one drops. Attachments are replaced and rebound. Equipping the worn set does nothing. A subject whose archetype lacks the set warns and is skipped; other subjects still switch. Two switches on one tick apply in reaction order, so the last wins. Every listed set's attachment models load with the level, so a switch never waits on a load.
- **Replication.** The NPC snapshot record gains the equipped set's name, which a client resolves against its own descriptor for attachments; `active_weapon_archetype` on pawns is the precedent. Weapons, layers and the gate have no client consumer and stay host-only. Fire sounds and projectiles already carry their own identity. A client that lacks the named set draws the base attachments and warns once. This bumps `WIRE_VERSION` and `SNAPSHOT_VERSION`. The compatibility digest stays as it is: descriptors resolve locally by name (`entity_model.md` §7d precedent).
- **Hot reload.** The equipped set re-resolves by name. Layers recompose from base plus set under the existing match-by-name rule. Weapon edits reach live instances as today. A reload that drops the worn set warns, and the instance is left unequipped: base layers, gated attacks unavailable, attachments kept (mesh refresh is unsupported). Nothing re-rolls.
- **Load rules (both runtimes, twin behavior).** An empty list, a duplicated set, or a weight that is not finite and positive rejects load. `equipment` on a descriptor without `behavior` warns, and the block drops. A set weapon that is not a projectile weapon is disabled with the existing enemy-attack warning.
- **Non-goals.**
  - Drop on death, NPC pickup, scavenging and disarm: later Epic 10 work builds on `equip`.
  - Hitscan enemy weapons: the projectile-only enemy rule stands.
  - Per-set health max, resist, model or animations.
  - Generating FGD `choices` from descriptors: the FGD stays hand-written, and the key is a string.
  - Player use: player pawns keep `inventory.loadout`.
  - Replicating NPC weapon resource state.
- **Docs amended at promotion:**
  - `entity_model.md`: Components, Weapon vocabulary, §7c gate.
  - `scripting.md`: §2, §11, §12 group steps.
  - `networking.md`: NPC record, §Version gates history.
  - `build_pipeline.md`: §Custom FGD.
  - Human-facing: `docs/scripting-reference.md`, `docs/level_design.md`, `sdk/TrenchBroom/postretro.fgd`.

### Scripting surface
```ts
import { brain, defineAttack, defineEntity, defineEquipment, defineReaction, equipped, npcs } from "postretro";
import { armor } from "./defenses";

// Named by binding: the map key `equipment` reads "burstKit" / "marksmanKit".
export const burstKit = defineEquipment({
  weapons: ["enemy_rifle_burst"],
  attachments: { hand_r: "models/ar_4/model.gltf" },
});
export const marksmanKit = defineEquipment({
  weapons: ["enemy_rifle_long"],
  attachments: { hand_r: "models/ar_4_long/model.gltf", head: "models/visor/model.gltf" },
  layers: [{ layer: armor, capacity: 40 }], // replaces the base stack
});

const spray = defineAttack({ weapon: "enemy_rifle_burst" /* positioning, draws, release… */ });
const snipe = defineAttack({ weapon: "enemy_rifle_long" /* … */ });

export const grunt = defineEntity({
  canonicalName: "grunt",
  components: {
    // health, mesh, faction as for any NPC
    equipment: [{ set: burstKit, weight: 3 }, { set: marksmanKit }], // weight defaults to 1
    behavior: {
      initial: "idle",
      activities: {
        idle: { animation: "idle", motion: "hold" },
        spray: { animation: "shoot", motion: "chaseTarget", action: { attack: spray } }, // skipped without burstKit
        snipe: { animation: "shoot", motion: "hold", action: { attack: snipe } },
      },
      transitions: {
        idle: [
          { to: "snipe", when: equipped(marksmanKit).and(brain.hasTarget) },
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
| guard read | brain scope, name-keyed leaf | `@equipped.<name>` | `equipped(set)` | `equipped(set)` | n/a |
| group step | typed command | n/a | `npcs(…).equip(set)` | `npcs(…).equip(set)` | n/a |
| NPC record field | snapshot record, optional | presence flag + name string, as `active_weapon_archetype` | n/a | n/a | n/a |

## Acceptance
### Automated
**Per-instance equipment**
- [ ] Two placements of one archetype, keyed to different sets: each fires only its set's weapons, carries its set's layer stack and capacities, and draws its set's attachments merged over the base by socket.
- [ ] A set without `layers` keeps the base stack; a set with `layers` replaces it.
- [ ] An archetype without `equipment` fires every attack weapon and loads as today.

**Gate, both sides**
- [ ] With a weapon equipped, its attack fires. With it unequipped, the row is never taken: no turn held, no token requested, no release animation, no whiff, and the next row runs on the same decide tick.
- [ ] A contact attack fires under every set.
- [ ] `equipped(set)` reads true only for the worn set. Naming a set outside the archetype's list rejects load.

**Selection**
- [ ] With no key, the same seed gives every placement and spawned copy the same set across two loads, and spawn order does not change it.
- [ ] Over 1,000 drawn copies at weights 3:1, the first set's share is within ±5% of 75%. A single-set list always yields that set.
- [ ] A spawner key naming a set gives every copy that set. An unknown name on a placement or spawner warns, naming it, then draws.

**Runtime switch**
- [ ] A switch during a dropped weapon's burst cancels it, and no further shot from that weapon fires.
- [ ] A weapon kept across the switch keeps its ammo or heat. A kept layer keeps its value, clamped; an added layer arrives full; a removed layer drops.
- [ ] Equipping the worn set changes nothing.
- [ ] A subject lacking the set warns and is skipped while the others switch.
- [ ] Two switches on one tick leave the later one worn.

**Reload and load rules**
- [ ] Hot reload re-resolves the worn set by name. An edited set weapon or layer capacity reaches live instances. A dropped set leaves the instance unequipped with base layers and its gated attacks unavailable.
- [ ] Each load rule in Decisions holds in both runtimes: empty list, duplicate set, bad weight, `equipment` without `behavior`, non-projectile set weapon.

**Replication**
- [ ] A connected client draws each NPC with its host-chosen attachments at materialize, and again after a runtime switch.
- [ ] A client lacking the named set draws base attachments and warns once.

**Surface**
- [ ] The Scripting surface example runs as a `content/dev` fixture in both runtimes and lowers to the same IR.

**Capacity**
- [ ] Measure and report: AI tick cost on a 256-enemy mixed-set stress fixture against the same fixture with `equipment` removed, under `context/lib/testing_guide.md` §Resource bounds.
### Manual
- [ ] On a dev map, two placements of one class visibly carry different guns, and a trigger reaction swaps one's gun in hand.
- [ ] Two-client co-op at 150 ms RTT (`POSTRETRO_NET_LAG`): the client sees each NPC's host-chosen weapon, before and after a switch.

## Path
- Seams:
  - Attacks: `AttackParams.weapon` → `resolve_projectile_attacks` (already per entity, in `BrainEntityPrograms`). Filter it by the equipped set.
  - Row skip: `RowSelection::skips` / `TurnGate` in `graph_eval.rs`.
  - Cancellation precedent: `arbitrate_attack_turns`, which already cancels a burst whose attack stops resolving.
  - Weapon state: `AiWeaponControllers::sync` derives it from resolved attacks.
  - Spawn: `attach_descriptor_components` (placement key read, composition). Spawner: the per-copy loop in `spawn_resolved_spawners` and the key read in `entity_spawner.rs`; `next_spawn_count` is claimed after attach today.
  - Draw seed: `behavior_draw_seed` / `BehaviorDrawSalt`.
  - Layers: `DefenseLayersComponent::from_descriptor` / `refreshed`. `DefenseParameters` is keyed by canonical name, so set expression layers need their own key.
  - Attachments: `MeshComponent::with_attachments`, rebind via `resolve_mesh_entity_bindings_for_entities`. Model upload: `distinct_mesh_models` / `resolved_spawner_mesh_models`.
  - Wire: `RawEntityRecord`, `active_weapon_archetype` and its validator. Client: `materialize_net_mesh_presentation`.
  - Guard read precedent: `attackTurnFree`. Group step precedent: `applyStatus`.
- Shape chosen: per-instance equipped set filtering per-entity attack resolution, with an engine row skip. Strongest rival: slot attacks (one row fires whatever is equipped); rejected because weapons differ in reach and cadence, so behavior differs by weapon.
- First slice: per-entity filtering plus the row skip, on two placements in single player, before wire or switch work. It falsifies the riskiest assumption: that a skipped attack row leaves turns, tokens and clip moments clean.
- Files past ~800 lines this extends: `data_archetype.rs`, `net_descriptor.rs`, `wire.rs`, `graph_eval.rs`, `spawner.rs`, `replication.rs`, `brain_programs.rs`, `compute.rs`. Split a cohesive block out first where this adds one, in its own behavior-preserving commit.
- Derivation and verified facts: `research.md`.

## Open questions
- A placement with no `equipment` key draws by weight, as a spawner copy does. The alternative defaults to the first listed set, so authored encounters stay fixed unless asked. Recommend the draw. — owner — **blocks build**
- `npcs(...).equip(set)` ships in this brief, as the proof that gating and the replicated field hold at runtime. The alternative ships placement-only and leaves the step to the drop/disarm work. Recommend shipping it. — owner — **blocks build**
- Whether set expression-capacity layers bind per archetype and set, or are refused for now. — **delegated**
- The tick on which a switch cancels a running burst relative to the fire proposal. — **delegated**
