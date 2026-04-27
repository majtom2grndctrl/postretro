# Scripting Architecture in Contemporary Modding Platforms

> **Purpose:** Document how real shipped mods structure scripted behavior — what ends up as data, what ends up as logic, how the boundary is enforced, and what performance problems emerge and how they are addressed.
> **Sources:** Factorio (Logistic Train Network, Angel's Mods, Krastorio2), GMod (ArcCW, ARC9-COD2019, Beatrun), Roblox/Luau (The Dungeons, Simple Combat Framework). All repos publicly available on GitHub.
> **Scope:** Architecture patterns, data/logic separation, content generation strategies, runtime performance and mitigation. Not a survey of scripting language features.

---

## 1. Platforms and Sources

| Mod | Platform | Language | Domain |
|-----|----------|----------|--------|
| Logistic Train Network (LTN) | Factorio | Lua | Train dispatch, logistics |
| Angel's Mods | Factorio | Lua | Overhaul — ores, fluids, recipes |
| Krastorio2 | Factorio | Lua | Overhaul — equipment, buildings |
| ArcCW | GMod | Lua | Weapon framework base |
| ARC9-COD2019 | GMod | Lua | 95-weapon CoD roster |
| Beatrun | GMod | Lua | Parkour movement, XP |
| The Dungeons | Roblox | Luau | RPG combat, weapons, stats |
| Simple Combat Framework (SCF) | Roblox | Luau | Combat framework base |

Factorio is the most studied because its engine enforces the data/logic boundary at the API level. GMod and Roblox are permissive — the separation is convention-driven — making their patterns more instructive for engines that don't enforce it.

---

## 2. The Universal Data/Logic Split

Every well-maintained mod in every ecosystem physically separates data definitions from runtime logic into distinct directories. The naming varies; the structure does not.

| Mod | Data directory | Logic directory |
|-----|---------------|-----------------|
| LTN | `prototypes/` | `script/` |
| Angel's Mods | `prototypes/` | `src/` |
| Krastorio2 | `prototypes/` | `scripts/` |
| ArcCW | SWEP table fields (inline) | `sh_firing.lua`, `sh_reload.lua`, etc. |
| ARC9-COD2019 | Per-weapon files in `lua/weapons/` | `arc9_cod2019_base.lua` framework |
| Beatrun | `sh/!Helpers.lua` (XP tables, constants) | `sv/XP.lua`, `sh/Climb.lua`, etc. |
| The Dungeons | `Shared/Configs/`, `Shared/Constants/` | `Server/Features/` |
| SCF | `Combat/AttackModule/Types.luau` | `Combat/AttackModule/init.luau` |

In Factorio this split is enforced by the engine. `data.lua` and its required files run at load time and cannot call runtime APIs. `control.lua` and its required files run during gameplay and cannot modify prototypes. The engine creates the boundary; mod authors fill the buckets.

In GMod and Roblox the split is purely conventional — nothing prevents mixing data and logic in the same file. Mods that mix them become hard to maintain; the ones studied here maintain the split anyway as a practice.

### What data files contain

Data files are 10–200 lines each. They contain only table literals: no event registration, no state, no function calls except pure helpers (color math, icon generation). The content is fully declarative:

- Item and weapon stats (damage, range, stack size, ammo type)
- Recipe definitions (ingredients, results, crafting time)
- Entity graphics, sounds, icon paths
- Technology tree nodes and prerequisites
- Localization strings
- Signal and virtual type definitions
- User-facing settings and defaults
- Type schemas (Luau `export type` declarations) — SCF defines `AttackData` as a 6-field type in a standalone `Types.luau`, imported by the runtime module as the enforced contract between data authors and logic authors

The Lua or Luau is used for its authoring convenience — comments, named constants, `require()` composition — but the files carry no behavior. They could be JSON without losing anything semantic.

### What logic files contain

Logic files are 150–2000 lines each. They contain event handlers, state machines, and algorithms that can't be expressed as parameters:

- Event handlers (`on_built_entity`, `on_entity_died`, `on_tick`, `on_player_crafted_item`)
- State machines (train delivery lifecycle, weapon heat/jam states, reload phases)
- Matching and dispatch algorithms (LTN supply-to-demand routing)
- Entity relationship tracking (tesla coil beam targets, roboport mode state)
- Network synchronization between client and server (GMod shared/server/client split)
- Dynamic entity interactions (reading circuit network signals, adjusting based on neighbors)

LTN's `script/dispatcher.lua` — 500+ lines of supply-demand matching — is the canonical example. The mod's entire behavioral purpose is in that file. Nothing about the matching algorithm can be reduced to a parameter.

---

## 3. Data Generation Patterns

Data files don't stay flat. As mods grow, repetition drives a predictable progression from hand-written tables to generator-driven output.

### Stage 1 — Hand-written flat tables

Small mods write every entry by hand. Ten to fifty items with the same shape are readable and maintainable as literal tables. No abstraction is needed or warranted.

```lua
data:extend({
  { type = "item", name = "iron-gear",   stack_size = 100, icon = "..." },
  { type = "item", name = "copper-gear", stack_size = 100, icon = "..." },
  { type = "item", name = "steel-gear",  stack_size = 50,  icon = "..." },
})
```

### Stage 2 — Helper functions for shared shape

When the same table structure appears five or more times with only values changing, a helper function collapses the repeated shape. The call site reads like data; the shape lives in one place.

```lua
local function make_gear(material, stack, tint)
  return {
    type = "item",
    name = material .. "-gear",
    stack_size = stack,
    icon_tint = tint,
    -- 8 more fields identical across all gears
  }
end

data:extend({
  make_gear("iron",   100, { r=0.8, g=0.8, b=0.8 }),
  make_gear("copper", 100, { r=1.0, g=0.5, b=0.2 }),
  make_gear("steel",   50, { r=0.6, g=0.6, b=0.7 }),
})
```

### Stage 3 — Config table plus generator loop

When content is combinatorial — multiple axes that each produce valid entries — the config list separates from the generator entirely. Angel's Mods uses this pattern pervasively: ores can be processed with multiple liquids through multiple process stages, producing a cross-product of recipes.

```lua
local ores    = { "iron", "copper", "lead", "tin", "nickel" }
local liquids = { "water", "sulfuric-acid", "angels-fluoxene" }

for _, ore in pairs(ores) do
  for _, liquid in pairs(liquids) do
    data:extend({ make_ore_recipe(ore, liquid) })
  end
end
```

Writing this as literal tables would be hundreds of entries. The config lists (axes) stay readable; the generated output is invisible. Angel's `recipe-builder.lua` extends this further with operator-based transformations (`=`, `+`, `*`, `~`) so recipes can express "add 10% to whatever the base ingredient amount is" without enumerating every modified variant.

### When to use each stage

The generator pattern only applies when differences between entries are values, not structural decisions. The transition from Stage 1 to Stage 2 happens around 5–10 entries with the same shape. The move to Stage 3 is warranted when the config list grows long enough to separate cleanly from the generator, when content must be conditionally generated based on external state (Angel's Mods does extensive `if mods["bobplates"] then` gating for inter-mod compatibility), or when non-programmers need to add entries without touching generator code.

ARC9-COD2019 illustrates when Stage 3 is the wrong tool: 95+ weapon files, each 1,820+ lines, with no generator loops. Two assault rifles (M4 and AK-47) share identical field structure, but each was individually tuned — different RPM, different recoil curves, different animation timings — as qualitative design decisions, not parameter variations. The AK-47 isn't a rescaled M4; it's a different weapon that happens to fit the same envelope. Large mods often use both approaches — generators for systematic content (recipes, tiers, elemental variants) and hand-written files for individually designed content (base weapon archetypes, unique items).

---

## 4. The Hybrid Pattern — Mostly Data with Callbacks

ArcCW (GMod weapon base) and Krastorio2 (Factorio) both demonstrate a middle ground between pure data files and pure logic files: a definition that is predominantly a data table but contains optional behavior callbacks.

### ArcCW weapon definitions

An ArcCW weapon file is ~80–85% data fields and ~15–20% optional callback hooks. The data fields cover ballistics, firing mechanics, accuracy, recoil, animations, and audio — everything that describes what the weapon is. The callbacks override default behavior where needed:

```lua
-- ~150 data fields covering ballistics, mechanics, accuracy, recoil, audio
SWEP.PrintName        = "M4A1"
SWEP.Primary.Damage   = 32
SWEP.Primary.Delay    = 60/800   -- 800 RPM
SWEP.Primary.ClipSize = 30
SWEP.AccuracyMOA      = 2.5
SWEP.Recoil           = 0.4
SWEP.ShootSound       = "weapons/m4a1/fire.wav"
-- ...

-- A weapon needing custom behavior adds a callback; most weapons omit these entirely
SWEP.Hook_TranslateAnimation = function(self, anim)
    -- remap animation names for this model's rig
end
```

All firing, reload, animation dispatch, and network sync logic live in separate framework modules (`sh_firing.lua`, `sh_reload.lua`, `sh_anim.lua`, etc.). A weapon modder who doesn't need custom behavior writes zero callbacks — the hooks are absent, not set to nil. The 15–20% callback share is a ceiling for the most complex weapons. The 95-weapon ARC9-COD2019 roster uses zero callbacks across all weapons.

### Krastorio2 equipment with triggered behavior

Krastorio2's tesla coil demonstrates the same hybrid at the equipment level. The data file defines what the entity is — health, resistances, collision bounds, animations. A separate script file defines what it does when triggered:

- `on_built_entity` — create companion turret and collision entities
- `on_script_trigger_effect` (fire event) — create beam connections between targets, transfer 3 MW power at 1.8× loss multiplier
- `on_player_placed_equipment` — track energy absorber targets

The script maintains four state tables (beams, turrets, towers, targets). None of that state could live in the data file — it's conditional, entity-relationship-driven, and tick-updated.

The energy absorber is a tighter example: a one-slot constraint (only one allowed per grid) that cannot be expressed as data. The entire script is one event handler, ~15 lines:

```lua
-- on_player_placed_equipment:
-- if equipment type == "energy-absorber" and grid already has one:
--   remove new one, return to cursor, show error (rate-limited to 30 ticks)
```

The item's stats, icon, and grid category are data. The constraint is a short script.

---

## 5. The Load-Time vs. Runtime Boundary

The data/logic split describes *what* goes where. A separate question is *when* the script/engine boundary is crossed — and this has direct consequences for performance and complexity.

### Load-time boundary (descriptor handoff)

The lowest-cost boundary crossing is one that happens once, at load or creation time. Script computes whatever it needs — sorting, centroid calculations, phase offsets, stat rolls — produces a complete descriptor, hands it to the engine, and is done. The engine owns all runtime behavior from that point forward with zero per-frame script involvement.

This pattern appears whenever a mod pre-configures a complex behavior sequence at setup time. A light animation system might take a list of entities, compute sine-pulse brightness arrays and per-entity phase offsets from their spatial positions, then hand the engine a fully-specified animation descriptor. The engine runs the animation loop internally; the script never fires again during that level's lifetime.

The same shape applies to item generation in loot-driven mods: RNG, pool sampling, affix selection, and stat scaling all run at item-drop time in script, producing a descriptor that the engine instantiates. The script's job ends at creation; the engine owns the weapon instance and its runtime behavior.

The cost profile of this boundary: a brief computation at load or creation time, then zero ongoing overhead. The descriptor functions as data that happened to require computation to produce.

### Runtime boundary (event callbacks)

Some behaviors genuinely cannot be pre-computed. A triggered effect that fires when a specific condition is met at an unpredictable time — an on-hit proc, a proximity trigger, a state transition driven by player input — requires the script to be re-invoked when the condition occurs.

Krastorio2's tesla coil is a clean example: which entities are in range and connected by beams changes as the game world changes. The script handler re-evaluates on each trigger event. This isn't avoidable by pre-computing a descriptor, because the relevant world state isn't known at load time.

The cost profile of this boundary: proportional to event frequency and the number of entities involved. At low entity counts — the scale of Doom or Quake — per-event script callbacks are not a practical concern. QuakeC ran all of Quake's game logic (enemy AI, weapon behavior, damage, effects, triggers) through an interpreted bytecode VM on 1996 hardware. Roblox and GMod both run large amounts of per-event script logic at scale without the pattern being the bottleneck.

### The practical question

For any scripted behavior, the question is: can it be fully expressed as a descriptor produced once, or does it require re-evaluation when runtime conditions change? The former is always cheaper; the latter is necessary when the behavior is genuinely condition-dependent. Most content falls into the former category. Triggered effects and AI-driven behavior are the minority cases that require runtime callbacks.

---

## 6. Performance: Documented Failure Modes and Mitigations

Three failure modes recur across all platforms studied, each with a documented mitigation.

### Polling → event-driven replacement

**Problem:** A mod registers an `on_tick` handler and iterates over all tracked entities every frame checking conditions. At low entity counts this is invisible. At scale — hundreds of tracked entities, multiple mods each doing the same — it compounds into meaningful UPS loss.

**Mitigation:** Register handlers for specific state-change events. Cost becomes proportional to event frequency, not entity count — zero cost when nothing is changing. Factorio's HandyHands mod is a documented case: replacing a per-tick crafting queue poll with `on_player_crafted_item` callbacks eliminated idle CPU entirely.

The related pattern is the **rolling event queue**: update an entity when it starts doing something, when it finishes, and every N ticks if still running. Most entities spend most of their time idle; this collapses "check 3,000 entities × 60 ticks/sec" into "check only entities currently transitioning state."

When periodic checks on many entities are genuinely necessary, distribute them across ticks by entity ID (Factorio's nth-tick bucket pattern):

```lua
script.on_nth_tick(64, function(event)
    local bucket = all_entities[event.tick % 64]
    for _, entity in pairs(bucket) do update(entity) end
end)
```

Each entity is checked once every 64 ticks rather than every tick. Documented recommendation: use prime-number intervals (61, 67, etc.) rather than powers of two — prime intervals have a larger LCM with other mods' intervals, preventing multiple mods' work from bunching on the same tick.

Quantified reference: switching from `settings.global` API calls to cached `storage[]` references improved a test save from 0.090 to 0.070 UPS. Factorio's baseline: 1 ms of scripting cost per tick equals ~3.5 UPS loss at 60 UPS.

### Per-frame allocation → object pooling

**Problem:** Creating and discarding tables or objects in tight loops generates continuous GC pressure. Luau's incremental collector handles most of this incrementally, but the atomic step — one indivisible phase per GC cycle — can cause 10–50ms pauses if the heap is large. In GMod and Roblox, frequent `Instance.new()` and `:Destroy()` cycles for bullets, particles, and UI elements are the primary source. Creating a new table per bullet fired is the canonical failure mode.

**Mitigation:** Pre-allocate a fixed set of objects and recycle them rather than creating and destroying per use:

```lua
local pool = ObjectPool.new(bulletTemplate)
local bullet = pool:Get()    -- reuse or create
-- ...
pool:Return(bullet)          -- reset and recycle, no GC involved
```

Roblox's own core scripts include an `ObjectPool.lua`. The pattern eliminates the GC assist cost that accumulates when allocation rate is high. For API calls specifically, cache results that don't change mid-tick rather than re-querying on every iteration:

```lua
-- Expensive: re-queries surface on every iteration
for _, entity in pairs(entities) do
    if entity.valid and entity.surface.name == target_surface_name then ...

-- Cheaper: resolve once before the loop
local surface = game.surfaces[target_surface_name]
for _, entity in pairs(entities) do
    if entity.valid and entity.surface == surface then ...
```

### Event connection leaks → Maid/Janitor pattern

**Problem:** In Roblox/Luau, event connections that aren't explicitly disconnected remain active indefinitely, even after the objects they reference are destroyed. Orphaned connections fire on every relevant event, accumulating CPU cost invisibly over a session's lifetime. This is the documented #1 memory and performance leak source in Roblox development.

**Mitigation:** A cleanup object (Maid or Janitor) manages all event connections for a component's lifetime:

```lua
local maid = Maid.new()
maid:GiveTask(part.Touched:Connect(onTouched))
maid:GiveTask(player.CharacterAdded:Connect(onSpawn))
maid:GiveTask(someInstance)

maid:Destroy()  -- disconnects connections before destroying instances
```

Connections are disconnected before instances are destroyed, preventing callbacks from firing on partially-destroyed state during cleanup. This is standard practice in professional Roblox development.

### Heavy computation → Actor parallelism (Roblox)

**Problem:** Expensive computation — raycasting, pathfinding, NPC AI — blocks the main script thread, causing frame spikes.

**Mitigation:** Roblox added first-class parallel execution via Actor instances:

```lua
task.desynchronize()              -- enter parallel phase
local result = expensiveRaycast() -- runs concurrently with other Actors
task.synchronize()                -- return to serial for data model writes
instance.Value = result
```

Read-only work runs on separate threads; writes must be serial. The engine enforces the constraint.

---

## 7. Engine Evolution Over Time

Both ecosystems show the same arc: performance problems that couldn't be solved in script were eventually absorbed into the engine.

**Factorio 2.0** made read-only operations on belts, combinators, and roboports run in parallel automatically. Mod authors changed nothing; the engine became smarter about when parallel execution was safe. Roboports gained an idle state — scripting cost dropped from 1 ms to 0.025 ms per tick on production saves, a 3.6% overall UPS gain. Cargo pod operations improved by 187–687×.

**Roblox Parallel Luau** was added after enough large games hit the single-threaded script ceiling. Scripts that needed parallelism no longer had to work around the limitation — the engine gave them a new primitive.

The pattern: when a scripted behavior consistently appears across many mods and consistently causes the same performance problem, that's the signal that the behavior belongs in the engine. The HandyHands mod's tick polling became an event subscription. The iterative entity scan that every mod reimplemented by hand became `surface.find_entities_filtered()` — a native API call with spatial indexing the engine already maintains. Each time, the pattern moved from script into the engine and the script was deleted.

---

## Non-Goals

This document does not cover:
- Scripting language feature comparisons (Lua vs. Luau vs. JavaScript vs. Python)
- Multiplayer or networked scripting architectures
- Shader or GPU-side scripting (material systems, compute shaders)
- Asset pipeline scripting (build-time tools, exporters)
- AI/ML-driven behavior generation
