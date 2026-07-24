# combat-demo — impact lifecycle, health + damage, and enemy AI / pathfinding

DEMO CONTENT exercising two M10 loops end to end:

1. **Impact-derived lifecycle.** A descriptor-declared health+hitbox entity is
   placed in a map and hit by the shipped weapon. A **mod-global** impact policy
   chooses what lethal damage means: ordinary lethal damage downs the dummy and
   queues recovery; a follow-up hit gibs it. Only removal reports a
   kill, so killing a fraction of the tagged dummies fires a `progress` event that
   drives an `applyDamage` reaction on the player, and the readonly
   `player.health` HUD slot drops.

2. **Zombie enemy lifecycle + pathfinding.** The `reference_enemy` (far east)
   uses its authored death animation while downed, recovers after a delay, and
   sits with the `player_spawn` (far west) at opposite ends of one large arena, with
   three free-standing full-height pillars strung along the centerline between
   them. The straight line from the enemy to the player is blocked by the center
   pillar, so the enemy must route AROUND it — A* over the baked navmesh regions,
   then a Simple-Stupid-Funnel string-pull (the funnel waypoints). Walk out from
   spawn and the enemy detours around the obstacle to reach you, instead of
   charging straight through it. There is wide open floor (~208 units) north AND
   south of the center pillar, so the agent rounds it in the open and never gets
   stuck.

## Floor plan

Interior `x 0..1024`, `y 0..512`, floor `z=0`, ceiling `z=128` (top-down; x east,
y north) — one large arena, ~4× the open floor area of the old ~512×256 room
(each horizontal dimension doubled). Three free-standing, floor-to-ceiling pillars
(`x[256,320] y[200,296]`, `x[480,544] y[208,304]`, `x[704,768] y[216,312]`) sit
near the centerline. Every gap — pillar to wall and pillar to pillar — is **≥160
units** wide, so there are no narrow doorways, no S-turns, and no concave pockets
the agent capsule can wedge into.

```
  y=512  ################################################################
         #..............................................................#
         #..............................................................#
         #..........##..........##..........##..........................#
         #...P..d.d..##....d.....##..........##.....................E....#
         #...........##..........##..........##.........................#
         #..............................................................#
         #..............................................................#
  y=0    ################################################################
         x=0    256 320   480 544   704 768                          1024

  # = wall / pillar   . = floor   P = player_spawn   E = reference_enemy   d = dummy
  WEST pillar x[256,320]   CENTER pillar x[480,544] (on the P->E line)   EAST pillar x[704,768]
  Route: P -> detour north OR south of the center pillar (~208 units clear) -> E.
```

### Emissive panels (bloom A/B)

Two floating `16 × 128 × 80` panels sit west of the pillars, both at
`x[192,208] z[32,112]`, directly ahead of the player spawn:

| Panel | Y extent | Texture | `_e` peak | Emissive term | Blooms? |
|---|---|---|---|---|---|
| Bright | `y[192,320]` | `neon/neon_glow_panel` | sRGB 255 | 4.0 | yes — 4× threshold |
| Dim | `y[352,480]` | `neon/neon_dim_panel` | sRGB 130 | 0.893 | no — just under threshold |

The pair is the A/B for emissive-with-bloom vs emissive-without-bloom. There is no
per-material bloom flag; bloom is decided purely by whether a fragment's linear
luminance clears `BLOOM_THRESHOLD` (`1.0`, `renderer/src/render/bloom.rs`). Both
panels use `Material::Neon`'s `emissive_strength` of `4.0`, so the authored `_e`
texel value is the only lever.

The dim `_e` is the bright one **clamped, not scaled**: most of the source pattern
already sits below the threshold (terms `0.42`–`0.79`) and blooms on nothing, so
only the 537 over-threshold texels are pulled down to `0.893`. Everything already
sub-threshold is left byte-identical, which keeps the panel's readable glow —
scaling the whole map instead crushes it to near-black. The clamp scales on the
peak channel like `soft_knee_tonemap` does, so clamped texels keep their hue.

Worth knowing: the *bright* panel already contains emissive-without-bloom. Only
537 of its 762 non-black texels clear the threshold; the rest glow without ever
entering the bright pass. The dim panel isolates that behavior across a whole
surface.

Note that bloom onset is gradual, not a cliff — the bright pass extracts
`1 - threshold/luminance` of a fragment, so a term of `1.1` yields a ~9% extraction
(~3% after `BLOOM_INTENSITY`), while the bright panel's `4.0` yields 75%.

Both panels share the same diffuse, and the west face each presents to the spawn
is unlit by direct light (the warm spots at `288 320` / `288 192` aim ±Y away from
them and fail `n·L`; the blue spot at `32 80` is outside its cone), so the
comparison isolates the emissive value rather than the lighting.

The bake reports **NavMesh: 18 regions, 22 portals** (53×105 cell grid @ 0.25 m) —
a genuinely multi-region, multi-portal mesh (the old single-room layout baked to
1 region / 0 portals, a straight-line chase). The floor is a single connected
walkable component (no area sealed off), so `find_path` always connects the two
spawns. Toggle the in-game nav overlay with **Alt+Shift+N** to see the regions,
portals, and the routed path: walk out and the enemy detours around the obstacle
to reach you.

## Files

- `content/dev/scripts/target-dummy.ts` — `defineEntity({ canonicalName:
  "target_dummy", components: { mesh, health: { max } } })`. The `max`
  HP ceiling makes it shootable; its skinned model supplies the targetable
  hit-zone capsules. Registered into the mod via
  `content/dev/start-script.ts`'s `ModManifest.entities`.
- `content/dev/models/decraniated_low_poly_retro_pixel/scene.gltf` — the visible
  skinned body. The current asset supplies one looping animation clip and
  torso/head/limb hit-zone capsules. The demo has no zone damage multipliers,
  so every successful hit uses the pistol's base damage.
- `content/dev/scripts/player.ts` — the player archetype, which carries
  `health: { max: 100 }` and DELIBERATELY no `hitbox` (the player is not
  ray-targetable; this also forecloses self-hit). Its HP is driven only through
  the level's named `applyDamage` reaction.
- `content/dev/scripts/combat-demo-reaction.ts` — the level **data script**
  (`setupLevel`). Returns a `progress` reaction over the `dummy` tag firing
  `dummiesCleared`, and an `applyDamage` reaction NAMED `dummiesCleared` targeting
  the `player` tag. Wired into the map via the worldspawn `data_script` KVP.
- `content/dev/scripts/combat-lifecycle.ts` — a **mod-global**
  `defineImpactEvent` registered from `start-script.ts`. `target_dummy` is
  exclusive to combat-demo, so it works when the map is opened from the catalog
  or directly by CLI while still composing with the level-local progress reactions.
- `content/dev/maps/combat-demo.map` — one large open arena (axis-aligned box
  brushes, plane style mirrored from `campaign-test.map`) with a `player_spawn`
  tagged `player` (far west), four `target_dummy` instances tagged `dummy` (just
  east of the player, in front of it), a `reference_enemy` tagged `enemy` and
  `combat-zombie` (far east), three free-standing full-height pillars near the centerline, and seven
  `light`s spread across the enlarged space. The center pillar blocks the straight
  player→enemy line, so the pathfinding has to route around it; the wide ≥160-unit
  clearance on every side keeps the agent from wedging. See the floor plan above.

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

The descriptor → `components.health` → model-authored hit-zone capsules → spawn → hitscan target →
`apply_damage` chokepoint → mod-global impact policy → authored lifecycle, end to end:

- Each `target_dummy` (max 30 HP) spawns standing in front of the player. Aiming
  the reference pistol (12 damage/hit) at one and firing **takes 12 HP per hit**,
  routed through the `apply_damage` chokepoint. Three torso hits down it
  (12 + 12 + 12 = 36 ≥ 30), but **do not remove it**: the mod-global policy queues
  `setHealth(maxHealth, { afterMs: 3000 })`. The target remains ray-targetable at
  zero HP, then re-arms when it recovers. This is the foundation for future
  stagger, revive, glory-kill, reward, and presentation policies.

- While a dummy is down, land a **fourth shot**. That follow-up hit reaches -12;
  the policy's level gate calls `despawn()`, so the dummy disappears and only then
  contributes its frozen kill credit to progress.
  There is no distinct down animation in the current single-clip fixture model;
  verify the down/recovery loop by waiting three seconds and downing it again.

- The far `reference_enemy` is the visual companion demo. Five body shots
  (60 HP at 12 damage per pistol hit) down it; its declared `death` state plays
  and its brain and navigation agent pause while the same three-second recovery
  is queued. Recovery immediately returns it to its idle pose, then normal AI
  resumes and selects its walk animation as it pursues. A sixth body shot while
  down reaches -12 and gibs it. The `combat-zombie` tag scopes this policy to
  this map, so other reference-enemy fixtures retain their existing behavior.

- **Model-authored hit zones.** The dummy uses the visible model's torso, head,
  arm, and leg capsules. Aim at the torso for the most reliable 12-damage pistol
  hits; the demo deliberately defines no zone damage multipliers.

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
- **Composition shape:** the map data script returns level-local `reactions`, while
  the dev mod's `ModManifest.events` contributes the impact policies. Their scopes
  compose at level install; the `dummy` and `combat-zombie` tags are exclusive to
  this map, so both policies work for catalog and direct CLI loading without
  changing other maps.
- **Descriptor placement:** a descriptor carrying `components.health` is directly
  map-placeable via `"classname" "target_dummy"`, resolved by the level loader's
  dispatch sweep against `canonicalName`.
