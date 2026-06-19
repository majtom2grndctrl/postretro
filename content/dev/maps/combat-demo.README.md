# combat-demo — M10 entity health + damage AND enemy AI / pathfinding demo

DEMO CONTENT exercising two M10 loops end to end:

1. **Health + damage.** A descriptor-declared health+hitbox entity is placed in a
   map, shot by the shipped weapon's hitscan ray, takes damage per hit through the
   `apply_damage` chokepoint, and despawns at zero HP — and killing a fraction of
   the tagged dummies fires a `progress` event that drives an `applyDamage`
   reaction on the player, so the player's HP (and the readonly `player.health`
   HUD slot) drops.

2. **Enemy AI + pathfinding.** The `reference_enemy` and the `player_spawn` sit in
   DIFFERENT walkable bays separated by two full-height interior walls with
   **offset doorways**. The straight line from the enemy to the player is blocked,
   so the enemy must route around corners — A* over the baked navmesh regions,
   then a Simple-Stupid-Funnel string-pull through the doorway portals (the funnel
   waypoints). Walk out from spawn and the enemy dog-legs through door A then door
   B to reach you, instead of charging in a straight line.

## Floor plan

Interior `x 0..512`, `y 0..256`, floor `z=0`, ceiling `z=128` (top-down; x east,
y north). Two interior wall brushes (`x[176,192] y[0,176]` and `x[320,336]
y[80,256]`, both floor-to-ceiling) carve the room into three bays. Each wall
leaves an ~80-unit doorway, and the two doorways are on OPPOSITE sides, forcing an
S-shaped dog-leg.

```
  y=256  ......................................................
         ...........#.............BAY 2.....#................E.
         ..P..d.....#..door A..............#####.door B......
         .....d.....#...(top gap)...........#................
         .....d.....#..WALL A..............WALL B............
         .....d.....#######.....(bottom gap)#................
  y=0    ......................................................
         x=0       176 192        320 336              512

  # = wall   . = floor   P = player_spawn   E = reference_enemy   d = dummy
  WALL A doorway = TOP gap (y 176..256);  WALL B doorway = BOTTOM gap (y 0..80)
  Route: P -> up through door A -> across BAY 2 -> down through door B -> E.
```

The bake reports **NavMesh: 9 regions, 8 portals** (53×27 cell grid @ 0.25 m) —
a multi-region, multi-portal corridor (the prior single-room layout baked to
1 region / 0 portals, a straight-line chase). Player (bay 1) and enemy (bay 3) are
in different regions; the 8 portals join all 9 regions into one connected
walkable component, so `find_path` always connects the two. Toggle the in-game nav
overlay with **Alt+Shift+N** to see the regions, portals, and the routed path.

## Files

- `content/dev/scripts/target-dummy.ts` — `defineEntity({ canonicalName:
  "target_dummy", components: { mesh, health: { max, hitbox, zoneMultipliers } } })`.
  The `max` HP ceiling makes it shootable; `zoneMultipliers` scales damage by
  where the ray lands. Registered into the mod via `content/dev/start-script.ts`'s
  `ModManifest.entities`.
- `content/dev/models/decraniated_low_poly_retro_pixel/scene.gltf` — the skinned
  body. Its joint nodes carry `extras` zone tags (`head`, `torso`, `arm`, `leg`
  with per-joint `hitZoneRadius`), making `target_dummy` a **zone-bearing** entity:
  the engine raycasts against posed bone capsules (broad-phased by a clip-derived
  bound), and the authored `hitbox` AABB is superseded. Only tagged joints
  register hits.
- `content/dev/scripts/player.ts` — the player archetype, which carries
  `health: { max: 100 }` and DELIBERATELY no `hitbox` (the player is not
  ray-targetable; this also forecloses self-hit). Its HP is driven only through
  the level's named `applyDamage` reaction.
- `content/dev/scripts/combat-demo-reaction.ts` — the level **data script**
  (`setupLevel`). Returns a `progress` reaction over the `dummy` tag firing
  `dummiesCleared`, and an `applyDamage` reaction NAMED `dummiesCleared` targeting
  the `player` tag. Wired into the map via the worldspawn `data_script` KVP.
- `content/dev/maps/combat-demo.map` — a three-bay walled room (axis-aligned box
  brushes, plane style mirrored from `campaign-test.map`) with a `player_spawn`
  tagged `player` (bay 1), four `target_dummy` instances tagged `dummy` (bay 1, in
  front of the player), a `reference_enemy` tagged `enemy` (bay 3, behind the two
  offset walls), and three `light`s (one per bay). The two interior walls with
  offset doorways are what make the navmesh multi-region/multi-portal so the
  pathfinding has to route around corners. See the floor plan above.

## Compile

```bash
# Compile the map (also compiles + embeds the data script via its data_script KVP)
cargo run -p postretro-level-compiler -- content/dev/maps/combat-demo.map -o content/dev/maps/combat-demo.prl
```

The mod entry script (`start-script.ts`, which imports the dummy descriptor) is
auto-compiled by the engine at startup in debug builds. To bundle it manually:

```bash
cargo run -p postretro-script-compiler --bin scripts-build -- --in content/dev/start-script.ts          --out /tmp/start-script.js
cargo run -p postretro-script-compiler --bin scripts-build -- --in content/dev/scripts/combat-demo-reaction.ts --out /tmp/combat-demo-reaction.js
```

## Run

```bash
cargo run -p postretro -- content/dev/maps/combat-demo.prl
```

## What this demo proves

The descriptor → `components.health` (hitbox authored; superseded by posed-bone capsules for zone-bearing entities) → spawn → hitscan target →
`apply_damage` chokepoint → death sweep path, end to end:

- Each `target_dummy` (max 30 HP) spawns standing in front of the player. Aiming
  the reference pistol (12 damage/hit) at one and firing **takes 12 HP per hit**,
  routed through the `apply_damage` chokepoint, and the dummy **despawns on the
  third hit** (12 + 12 + 12 = 36 ≥ 30). Seeing it vanish is the proof that the
  posed-bone capsules (zone-bearing path) made it ray-targetable, the damage flowed
  through the chokepoint, and the death sweep despawned it at zero HP.

- **Hit zones (M10 skeletal hit zones).** Because the model's joints are tagged,
  damage scales by where you hit: a **headshot deals 2.5×** (12 → 30, a one-shot
  kill of the 30-HP dummy), a **leg shot 0.5×** (12 → 6), and torso/arm hits apply
  1.0× (12). Aiming at the head vs. a leg and watching the HP drop differ — or the
  dummy drop in one headshot vs. three torso shots — is the proof the posed-capsule
  raycast and per-zone multiplier are live. Note hits register only on the tagged
  limbs (head/torso/arms/legs); a ray between limbs misses even inside the body's
  bounding box (the two-phase narrow test). There is no in-game capsule overlay —
  verification is by observed damage, not a debug draw.

- The `progress` reaction's denominator (4 tagged dummies) is captured at level
  load. At `at: 0.5`, killing **two** dummies crosses the threshold and fires the
  `dummiesCleared` event exactly once (a one-shot — further kills do not re-fire).

- `dummiesCleared` is dispatched through the **death-event drain**
  (`fire_named_event_with_sequences`) — the only drain that invokes primitive
  reaction handlers. It matches the `applyDamage` reaction registered under the
  same name, which routes **35 damage** to the `player`-tagged pawn. The player's
  HP drops from 100 to 65, and the readonly `player.health` HUD slot follows.

## Why the chain is `progress → named event → applyDamage`, not a simpler trigger

- A `levelLoad` reaction fires **before the first rendered frame**, so an
  `applyDamage` hung off it would drop HP invisibly — and nothing is dead yet.
  The damage has to be gameplay-driven, hence the `progress` trigger.
- The plain `fire_named_event` drains (the movement/weapon event names) never
  invoke primitive handlers. Only an event routed through the death-event drain
  reaches the `applyDamage` handler. A `progress` `fire` goes through that drain,
  so the event name it fires must match the `applyDamage` reaction's name.

## Tag discipline

The `dummy` tag is **exclusive to the target dummies**. The progress denominator
counts every entity carrying the tag, so a shared tag would skew the ratio —
e.g. tagging the player `dummy` too would make `at: 0.5` require killing 2 of 5,
and the player can't be killed by the weapon (no hitbox), so the threshold could
never be reached. The player gets its own `player` tag, matched only by the
named retaliation reaction.

## Authoring notes / caveats

- **Player-start classname:** `player_spawn` (confirmed in
  `sdk/TrenchBroom/postretro.fgd` and `build_pipeline.md` §Built-in Classname
  Routing).
- **`_tags` on `player_spawn`:** the spawn path forwards the parsed `_tags` list
  onto the spawned player pawn (`spawn_descriptor_instance` →
  `try_spawn(transform, &entity.tags)`), so `"_tags" "player"` lands on the pawn
  and the `applyDamage` reaction's `tag: "player"` resolves to it.
- **Reaction trigger shape:** reactions are surfaced through `setupLevel`'s
  returned `LevelManifest` (`{ reactions }`), NOT through the mod manifest. This file
  uses a `ProgressReactionDescriptor` (`{ progress: { tag, at, fire } }`) and a
  `PrimitiveReactionDescriptor` (`{ primitive, tag, args }`).
- **Descriptor placement:** a descriptor carrying `components.health` is directly
  map-placeable via `"classname" "target_dummy"`, resolved by the level loader's
  dispatch sweep against `canonicalName`.
