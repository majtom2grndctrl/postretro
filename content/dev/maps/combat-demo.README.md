# combat-demo — impact lifecycle, resource grants, health + damage, and enemy AI / pathfinding

DEMO CONTENT exercising four connected paths end to end:

1. **Impact-derived lifecycle.** A descriptor-declared health+hitbox entity is
   placed in a map and hit by the shipped weapon. A **mod-global** impact policy
   chooses what lethal damage means: ordinary lethal damage downs the dummy and
   queues recovery; its authored raw-overkill predicate can gib it. Only removal reports a
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

3. **Limitator line-of-sight cover.** The ranged `limitator` starts with a clear
   view of the player, but a nearby static pillar lets the player break that
   sightline. Its authored graph returns from aim/fire to `close` after the
   shared debounced `brain.targetVisible` verdict falls, so the existing
   combat-slot movement can reposition and reacquire. The engine fire gate,
   rather than these authored edges, remains the damage/event authority.

4. **Reference resource grants.** The engine has no concept of a reward. This
   dev mod supplies two replaceable policies instead: a dummy kill-edge impact
   policy grants its damager 8 `shells.buck`, while a nearby trigger volume
   grants its entering activators 24 `shells.buck`. The walkthrough keeps the
   two entry points visibly distinct.

## Floor plan

Interior `x 0..1024`, `y 0..512`, floor `z=0`, ceiling `z=128` (top-down; x east,
y north) — one large arena, ~4× the open floor area of the old ~512×256 room
(each horizontal dimension doubled). Three free-standing, floor-to-ceiling pillars
(`x[256,320] y[200,296]`, `x[480,544] y[208,304]`, `x[704,768] y[216,312]`) sit
near the centerline. A fourth, narrower full-height cover pillar
(`x[128,160] y[288,352]`) sits north-east of the player spawn. The Limitator
starts south-east of it at `x=400, y=140`, so it can acquire the player before
the player steps north behind cover. Every gap — pillar to wall and pillar to
pillar — is **≥160 units** wide around the original reference-enemy route, so
there are no narrow doorways, no S-turns, and no concave pockets the agent
capsule can wedge into.

```
  y=512  ################################################################
         #..............................................................#
         #.......C......................................................#
         #..........##..........##..........##..........................#
         #...P.A.d.d..##....d.....##..........##.....................E....#
         #...........##..........##..........##.........................#
         #........................L.....................................#
         #..............................................................#
  y=0    ################################################################
         x=0   128 160 256 320  400 480 544   704 768               1024

  # = wall / pillar   . = floor   P = player_spawn   L = limitator
  # A = ammo pickup   E = reference_enemy   d = dummy   C = LOS cover pillar
  WEST pillar x[256,320]   CENTER pillar x[480,544] (on the P->E line)
  Route: P -> detour north OR south of the center pillar (~208 units clear) -> E.
```

### Limitator LOS-cover walkthrough (manual integration)

Let the Limitator acquire, approach, aim, and fire from its initially clear
south-east position. Then move north of the small `C` pillar (the player-start
side, around `y=352`) until the pillar fully spans the Limitator-to-player view.
After the fixed loss-grace window, player-health damage stops and the Limitator
leaves its aim/fire cycle for `close`; it uses the LOS-aware combat slots to route
around the pillar, reacquires, and resumes its normal cycle.

This is intentionally a manual composition check: nav candidate choice, route
timing, facing slew, and the fire latch make the exact tick and path unsuitable
for a map snapshot. The HUD does not expose individual `enemyAttack` events or
the grace counter, so this walkthrough cannot itself prove their absence; the
scripted AI tests are the runnable HP-and-event assertion. The existing dummy
and far `reference_enemy` demonstrations remain available to exercise corpse,
recovery, and despawn ordering independently of the cover interaction.

### Emissive panels (bloom A/B)

Two floating `16 × 128 × 80` panels share the `y[192,320] z[32,112]` band on the
player-spawn sightline, one near the west end and one by the east pillar:

| Panel | X extent | Texture | `_e` peak | Peak luminance×4 | Blooms? |
|---|---|---|---|---|---|
| Bright | `x[192,208]` | `neon/neon_glow_panel` | sRGB 255 | 4.0 | yes — 4× threshold |
| Dim | `x[688,704]` | `neon/neon_dim_panel` | sRGB 136 | 0.985 | no — genuinely sub-threshold |

The bright pass gates on **Rec.709 luminance** of the linear color, not the peak
channel — `luminance = dot(color, (0.2126, 0.7152, 0.0722))` in `bloom_extract.wgsl`.
The neon texels are near-neutral, so peak-channel and luminance nearly coincide.
**sRGB 136 is the highest 8-bit `_e` byte that stays under the threshold:**
`s2l(136)*4 = 0.985` (< 1.0), while the next byte `s2l(137)*4 = 1.0006` crosses it
(quantization has no byte at exactly 1.0). So 136 is where the threshold sits from
below — at it, `excess = max(luminance - 1.0, 0.0)` is zero for every texel and the
bright pass extracts nothing.

Each panel has a wide-cone `light_spot` at its west face
(`192 256 72` and `688 256 72`, `_cone 45` / `_cone2 85`, aimed `-15 180 0`).
**Emissive is a shader term only — it lights nothing** — so these stand in for the
spill the panels would cast. Their intensities track each panel's mean emissive
term (`0.372` vs `0.146`, hence `light 150` vs `60`). The light is co-planar with
its panel's west face, so `n·L` on that face is exactly zero and the spill adds
nothing to the panel's own fragment — the threshold comparison stays clean
regardless of cone width.

The pair is the A/B for emissive-with-bloom vs emissive-without-bloom. There is no
per-material bloom flag; bloom is decided purely by whether a fragment's linear
luminance clears `BLOOM_THRESHOLD` (`1.0`, `crates/renderer/src/render/bloom.rs`).
Both panels use `Material::Neon`'s `emissive_strength` of `4.0`, so the authored
`_e` texel value is the only lever.

The dim `_e` is the bright one **clamped, not scaled**: most of the source pattern
already sits below the threshold (terms `0.42`–`0.79`) and blooms on nothing, so
only the 515 texels whose peak channel exceeds the ceiling are pulled down.
Everything already sub-threshold is left byte-identical, which keeps the panel's
readable glow — scaling the whole map instead crushes it to near-black. The clamp
scales on the peak channel like `soft_knee_tonemap` does, so clamped texels keep
their hue.

Worth knowing: the *bright* panel already contains emissive-without-bloom. By the
luminance metric the bright pass actually uses, only 468 of its 762 non-black
texels clear the threshold (515 by peak channel — colored texels with one hot
channel can sit over per-channel but under in luminance); the rest glow without
ever entering the bright pass. The dim panel isolates that behavior across a whole
surface.

The dim panel is deliberately parked **just under** the threshold at sRGB 136
(`0.985`): zero texels clear the luminance threshold, so the bright pass extracts
literally nothing — a genuine no-bloom surface, not merely an imperceptible one.
136 is the reference for "as bright as an emissive surface can idle with no halo
at all"; 137 (`1.0006`) is the first byte that would cross. There is no measurable
perf saving either way — the bloom chain runs at fixed cost whenever bloom is
enabled — but keeping the panel fully under the line makes the no-bloom property
exact rather than approximate.

Bloom onset is a ramp, not a cliff — the bright pass extracts
`1 - threshold/luminance`, which is scaled again by `BLOOM_INTENSITY` (`0.35`)
when composited back:

| Emissive term | Extracted | After `BLOOM_INTENSITY` |
|---|---|---|
| ≤ 1.00 | 0% | 0% — no halo |
| 1.05 | 4.8% | 1.7% |
| 1.10 | 9.1% | 3.2% |
| 1.20 | 16.7% | 5.8% |
| 1.50 | 33.3% | 11.7% |
| 2.00 | 50.0% | 17.5% |
| 4.00 (bright panel) | 75.0% | 26.2% |

Useful for pulsing an emissive surface as state feedback: hold the idle state at
or below `1.0` for no halo, and drive the active state into the `1.1`–`1.5` band
for a halo that reads as a change without dominating the frame. This demo panel
idles at `0.985` — just `0.015` under the line — to mark exactly where the
threshold is; a shipping idle state wants more margin (`~0.9`) so indirect light
on the surface can't nudge it over. The panel is a measuring stick, not a template.

Both panels share the same diffuse, and the west face each presents to the spawn
is unlit by direct light (the flanking warm spots aim ±Y away from
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
  so every successful hit uses the shotgun's base damage.
- `content/dev/scripts/player.ts` — the player archetype, which carries
  `health: { max: 100, hitbox }`. Its body-spanning hitbox makes it a normal
  ray-targetable presence, while player weapon queries exclude their firing
  pawn to prevent a range-0 self-hit. In this demo, its HP is driven only
  through the level's named `applyDamage` reaction.
- `content/dev/scripts/combat-demo-reaction.ts` — the level **data script**
  (`setupLevel`). Returns a `progress` reaction over the `dummy` tag firing
  `dummiesCleared`, and an `applyDamage` reaction NAMED `dummiesCleared` targeting
  the `player` tag. It also binds the `ammo_pickup` trigger's enter edge to a
  24-`shells.buck` grant for its activators. Wired into the map via the
  worldspawn `data_script` KVP.
- `content/dev/scripts/combat-lifecycle.ts` — a **mod-global**
  `defineImpactEvent` registered from `start-script.ts`. `target_dummy` is
  exclusive to combat-demo, so it works when the map is opened from the catalog
  or directly by CLI while still composing with the level-local progress reactions.
  Its `ammo-on-kill` policy is reference content: it pays the damager 8
  `shells.buck` on a dummy kill edge, but a mod replaces the policy wholesale.
- `content/dev/maps/combat-demo.map` — one large open arena (axis-aligned box
  brushes, plane style mirrored from `campaign-test.map`) with a `player_spawn`
  tagged `player` (far west), four `target_dummy` instances tagged `dummy` (just
  east of the player, in front of it), a `reference_enemy` tagged `enemy` and
  `combat-zombie` (far east), a `limitator` with an initially clear player
  sightline (south-east), a touch-triggered `ammo_pickup_volume` just east of
  spawn, three free-standing full-height pillars near the centerline, and a
  smaller dedicated Limitator-cover pillar north-east of spawn. Seven `light`s
  spread across the enlarged space. The center pillar blocks the straight
  player→reference-enemy line, so that pathfinding has to route around it; the
  wide ≥160-unit clearance on every side keeps the agent from wedging. See the
  floor plan above.

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

- **Presentation fixture.** The dev mod globally registers floating damage
  numbers and a recently-damaged enemy health bar. Every hit on these dummies
  exercises the same `present()` and `damagedEnemies()` declarations used by
  any other dev map with the shared dummy/enemy impact policies. Damage
  numbers rise from the hit target; the bar lingers above it after a hit.

- The shot counts below assume a point-blank torso shot at **1 m or closer**.
  At that distance the 4° cone keeps every pellet within 7 cm of the aim ray, so
  all eight pellets connect. Each pellet deals 3 damage, making a full shell 24
  damage, routed through the `apply_damage` chokepoint.

- Each `target_dummy` (max 48 HP) goes **48 → 24 → 0** after two full-connect
  shells and downs on the second shell's final pellet, but **does not remove**:
  the mod-global policy queues
  `setHealth(maxHealth, { afterMs: 3000 })`. The target remains ray-targetable
  while down, then re-arms when it recovers. This is the foundation for future
  stagger, revive, glory-kill, mod-owned reward, and presentation policies.

- While a dummy is down, land a **third shell**. Its first pellet reads
  **0 → -3**, exactly meeting the authored `-3` raw-overkill predicate; the
  policy calls `despawn()`, so the dummy disappears, leaves the resurrection
  loop, and only then contributes its frozen kill credit to progress. This is
  one impact's unfloored `healthAfter`, not accumulated shell damage: stored HP
  stays at zero between pellets. There is no distinct down animation in the
  current single-clip fixture model; verify the down/recovery loop by waiting
  three seconds and downing it again.

- The finisher is content policy, not an engine corpse rule. A low-HP target can
  gib within one pellet fan when its own predicate crosses: at 3 damage per
  pellet, a 5-HP target goes **5 → 2 → -1**, then its next pellet reads
  **0 → -3** and this policy gibs it. An author who does not want gibbing omits
  or replaces the `despawn()` branch entirely.

- After the three-shell dummy demonstration, hold reload and watch the shotgun
  refill one `shells.buck` round every 450 ms. The shotgun holds eight rounds:
  the dummy's three shells and the enemy's four shells each fit within one
  magazine, but a reload deliberately separates those two encounters. Fire
  during that reload to see the in-flight shell step cancel; keep reload held
  through the shot to see it restart on the following tick.

- The far `reference_enemy` is the visual companion demo. At point blank, three
  body shells take its 70 HP through **46 → 22 → -2** and down it on the third
  shell's final pellet. That raw `-2` does not cross the authored `-3` predicate;
  its declared `death` state plays and its brain and navigation agent pause while
  the same three-second recovery is queued. Recovery immediately returns it to
  its idle pose, then normal AI resumes and selects its walk animation as it
  pursues. The first pellet of a fourth body shell reads **0 → -3** and gibs it.
  The `combat-zombie` tag scopes this policy to this map, so other
  reference-enemy fixtures retain their existing behavior.

- **Model-authored hit zones.** The dummy uses the visible model's torso, head,
  arm, and leg capsules. Aim at the torso for the most reliable full-connect
  shotgun shells; the demo deliberately defines no zone damage multipliers.

- The `progress` reaction's denominator (4 tagged dummies) is captured at level
  load. At `at: 0.5`, killing **two** dummies crosses the threshold and fires the
  `dummiesCleared` event exactly once (a one-shot — further kills do not re-fire).

- `dummiesCleared` is dispatched through the **death-event drain**
  (`fire_named_event_with_sequences`) — the only drain that invokes primitive
  reaction handlers. It matches the `applyDamage` reaction registered under the
  same name, which routes **35 damage** to the `player`-tagged pawn. The player's
  HP drops from 100 to 65, and the readonly `player.health` HUD slot follows.

## Resource-grant walkthrough (reference content)

The engine intentionally does not classify any event as a reward. The two grants
below are dev-mod reference content that another mod replaces with its own
policies; they exercise two distinct recipient paths.

1. **Kill payout — impact source.** At the point-blank distance above, shoot a
   `target_dummy` twice with the reference shotgun. The second full-connect shell
   crosses from positive health to below zero, so `ammo-on-kill` grants the
   damager **8 `shells.buck`**. This is a kill edge, not a corpse-hit level gate.
   `combatDummyLifecycle` resurrects a merely downed dummy after three seconds,
   so downing it again earns another 8-ammo payout. A third full-connect shell
   while it is down has a first pellet with raw `healthAfter = -3`, meeting the
   authored gib threshold instead: it removes that dummy from the loop and does
   not pay the kill edge.

2. **Volume payout — trigger activator.** From the player start, walk east through
   `ammo_pickup_volume` (the `A` in the floor plan). Its `onTriggerEvent` enter
   binding grants the entering player **24 `shells.buck`**, independently of
   combat. The touch volume uses `fire_mode: multiple` with a 3-second rearm and
   deliberately never self-disarms in v1, so leave it, wait three seconds, and
   enter again for another volume payout.

3. **Per-player XP alongside a shared team count.** Every dummy kill also awards
   **10 XP** to its damage source and increments the shared `teamKills` counter.
   In a two-client session, have each player kill a different number of dummies:
   each HUD shows only that player's XP, so the XP readouts diverge, while the
   shared team-kill count agrees for both clients. They are the same reward policy;
   the declaration and `.byPlayer(impact.source)` address make XP per-player,
   while the plain `teamKills` update remains one session pot. XP persists across
   sessions and travels with its player to a new host through the join seed;
   `teamKills` remains session-scoped.

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
e.g. tagging the player `dummy` too would make `at: 0.5` require killing 2 of 5.
The player is targetable, but their own weapon queries exclude the firing pawn,
so shooting cannot advance that extra progress entry. The player gets its own
`player` tag, matched only by the named retaliation reaction.

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
