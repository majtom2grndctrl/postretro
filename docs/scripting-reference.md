# Scripting Reference

## Mod entry point

Every mod has a single `start-script` at its root that runs once at engine init, before any level loads. This is where cross-level concerns — entity-type registration, game-wide setup — live.

**File location.** Place exactly one of these at the mod root (the `content/<mod>/` directory):

- `start-script.ts` — TypeScript source. Compiled to `start-script.js` automatically in debug builds; ship the compiled `.js` in release builds.
- `start-script.luau` — Luau source. Read directly.

If both `start-script.js` and `start-script.luau` exist, the engine errors at init.

If neither exists: in debug builds, the engine boots normally with no mod-declared types. In release builds, the engine errors at init.

**`setupMod()` contract.** The script must export a `setupMod()` function that takes no arguments and returns a `ModManifest`:

```typescript
// start-script.ts
import { registerEntity } from "postretro";
import { playerDescriptor } from "./actors/player";

registerEntity(playerDescriptor);

export function setupMod() {
  return { name: "MyMod" };
}
```

```lua
-- start-script.luau
local player = require("./actors/player")
registerEntity(player.descriptor)

function setupMod()
  return { name = "MyMod" }
end
```

`ModManifest` requires a `name` field (string). Additional fields will be added as the mod system grows. The engine errors at init if `setupMod` is missing, throws, returns a non-object, or returns an object without `name`.

**Imports and `require`.**

- TypeScript: standard ES module `import` of relative paths. The script compiler bundles all relative imports into `start-script.js` at build time. Bare-specifier imports of `"postretro"` symbols are stripped (the symbols arrive as runtime globals).
- Luau: `require("./path")` resolves relative to the mod root. `require("./actors/player")` reads `<mod_root>/actors/player.luau` (the `.luau` extension is appended automatically). `..` traversal and absolute paths are rejected. Module caching, init-file conventions, and upward search are not implemented.

**Lifecycle.** Entity types registered from `start-script` (and any domain scripts it imports) survive level loads — they live in the engine-global type registry. Reactions are not registered here; those belong in per-level data scripts via `setupLevel(ctx)`. The mod-init VM is dropped after `setupMod` returns; no script state persists past that point.

---

## registerEntity

`registerEntity(descriptor)` registers a script-defined entity archetype for use across all levels. Call this from a `start-script` (mod scope), before level load.

| Field | Type | Description |
|-------|------|-------------|
| `classname` | `string` | The `.map` classname this archetype matches. Must not conflict with a built-in classname (e.g. `billboard_emitter`) — built-ins take precedence and a warning is logged. |
| `components.emitter` | `ComponentValue` (optional) | Emitter component attached at spawn. Use `smokeEmitter`, `sparkEmitter`, or `emitter()`. |
| `components.light` | `{ color: [r, g, b], range: number, intensity: number, is_dynamic: boolean }` (optional) | Light component attached at spawn. Descriptor-spawned lights are always treated as dynamic regardless of `is_dynamic`. |

**Idempotency:** calling `registerEntity` again with the same classname and descriptor is a silent no-op. If the descriptor differs, the new one wins and a debug log is emitted.

**Archetype spawn order:** after built-in classname dispatch runs at level load, the engine sweeps `world.map_entities` a second time and spawns script-registered archetypes for any entity whose classname matched a `registerEntity` call and was not handled as a built-in.

**KVP overrides with `initial_` prefix:** any `initial_`-prefixed key on a `.map` placement (e.g. `initial_rate`, `initial_range`, `initial_is_dynamic`) overrides the matching descriptor field at spawn time. On parse failure the descriptor default is kept and a warning is logged. The key is `initial_` followed by the descriptor's field name (e.g. `initial_range` overrides `LightDescriptor.range`).

> **Naming note:** `BillboardEmitterComponent.initial_velocity` already starts with `initial_`, so the mechanical override key would be `initial_initial_velocity` (prefix doubled). Both `initial_initial_velocity` and the friendlier alias `initial_velocity` are accepted; either writes to `BillboardEmitterComponent.initial_velocity` at spawn. The shortest alias `velocity` is also accepted and writes the same field.

```typescript
registerEntity({
  classname: "exhaustPort",
  components: {
    emitter: smokeEmitter({ rate: 8, spread: 0.3, lifetime: 2.0 }),
  },
});

registerEntity({
  classname: "campfire",
  components: {
    light: { color: [1.0, 0.5, 0.1], range: 256, intensity: 1.2, is_dynamic: true },
    emitter: sparkEmitter({ rate: 4, spread: 0.5, lifetime: 0.8 }),
  },
});
```

---

## `components.health`

Attach a `health` block to an entity descriptor to give it hit points. An entity
with health can take damage through the engine's single damage chokepoint and is
removed by the death sweep once its HP reaches zero.

```typescript
defineEntity({
  canonicalName: "target_dummy",
  components: {
    mesh: { model: "models/grunt/scene.gltf" },
    health: {
      max: 30,
      hitbox: {
        halfExtents: [0.4, 0.9, 0.4],
        offset: [0, 0.9, 0],
      },
    },
  },
});
```

| Field | Type | Description |
|-------|------|-------------|
| `max` | `number` | Hit-point ceiling. Must be finite and `> 0` — otherwise the descriptor is rejected at load with a descriptive error. The component materializes with `current == max` at spawn. |
| `hitbox` | `{ halfExtents, offset? }` (optional) | One world-aligned AABB. **Present ⇒ the entity is hitscan-targetable** (a weapon ray can hit it and route damage through the chokepoint). **Absent ⇒ it cannot be ray-targeted at all.** Fixed per archetype. |
| `hitbox.halfExtents` | `[x, y, z]` | Box half-size on each axis, in meters. The engine is Y-up, so the middle component is the vertical half-height. Each element must be finite and `> 0`. |
| `hitbox.offset` | `[x, y, z]` (optional) | Shifts the box center from the entity's transform origin. Each element must be finite. A common use is lifting the box up by its half-height (e.g. `offset: [0, 0.9, 0]` for a `0.9` vertical half-extent) so it rises from a foot-level origin to span the body. |

**Why the hitbox is the targetability switch.** Carrying a hitbox is exactly what
makes an entity shootable. A shooting target declares both `max` and a `hitbox`.
The player pawn, by contrast, declares health with **no** hitbox — so the weapon
ray never targets the player (and a player can't shoot itself); the player's HP
is driven only through an `applyDamage` reaction.

**A health-bearing descriptor is map-placeable.** Like any `defineEntity`
archetype, an entity carrying `components.health` is placed by `canonicalName` —
`"classname" "target_dummy"` in a `.map` spawns one.

---

## setupLevel

Per-level data scripts export a `setupLevel(ctx)` function to register reactions and other level-scoped state. The engine calls it when the level starts; its effects apply only to that level.

---

## world.query

`world.query(filter)` returns an array of entity handles matching a filter. The concrete handle type depends on the `component` you query — `"light"` returns `LightEntity[]` and `"fog_volume"` returns `FogVolumeHandle[]`. Querying an unknown component name throws `InvalidArgument`.

```typescript
world.query({ component: "light" })            // all lights → LightEntity[]
world.query({ component: "light", tag: "foo" }) // only lights tagged "foo"
```

Providing a `tag` narrows the result to entities whose tag matches exactly.

### LightEntity

Returned when `component` is `"light"`. All fields are a snapshot at query time. `setAnimation` operates on the **live** entity by id and does not require a fresh `world.query`.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `EntityId` | Stable entity id. Pass to `setLightAnimation` and other primitives. |
| `transform.position` | `{ x, y, z }` | Light origin in world space at query time. |
| `isDynamic` | `boolean` | Whether the light is runtime-dynamic. Sourced from the `_dynamic` key in the `.map` file. Dynamic lights participate in the per-fragment GPU light loop and the shadow-slot scheduler; use this to gate color animations (color animation is only valid on dynamic lights). |
| `tags` | `string[]` | The entity's tags at query time. Empty array if untagged. |
| `component` | `LightComponent` | Full component snapshot at query time. See [LightComponent](#lightcomponent) below. |

#### Example — rolling wave down a hallway

Tag the hallway lights `"hallway_wave"` in TrenchBroom. The data script queries them, sorts along the x axis, and staggers `phase` so the pulse travels.

**TypeScript**

```typescript
import { world } from "postretro";
import type { LightAnimation } from "postretro";

const lights = world
  .query({ component: "light", tag: "hallway_wave" })
  .sort((a, b) => a.transform.position.x - b.transform.position.x);

const pulse: LightAnimation = {
  periodMs: 10000,
  brightness: [
    0.1, 0.1, 0.1, 0.1, 0.1,
    0.3, 0.8, 1.0, 0.8, 0.3,
    0.1, 0.1, 0.1, 0.1, 0.1,
    0.1, 0.1, 0.1, 0.1, 0.1,
  ],
};

lights.forEach((light, i) => {
  light.setAnimation({ ...pulse, phase: i / lights.length });
});
```

**Luau**

```lua
-- `world` is a bare global installed by the engine prelude — no require needed.
local lights = world:query({ component = "light", tag = "hallway_wave" })
table.sort(lights, function(a, b)
  return a.transform.position.x < b.transform.position.x
end)

local pulse = {
  periodMs = 10000,
  brightness = {
    0.1, 0.1, 0.1, 0.1, 0.1,
    0.3, 0.8, 1.0, 0.8, 0.3,
    0.1, 0.1, 0.1, 0.1, 0.1,
    0.1, 0.1, 0.1, 0.1, 0.1,
  },
}

for i, light in ipairs(lights) do
  light:setAnimation({
    periodMs = pulse.periodMs,
    brightness = pulse.brightness,
    phase = (i - 1) / #lights,
  })
end
```

---

## world.getGravity / world.setGravity

Read and write the world gravity at runtime. The starting value is set per-map via the `initialGravity` worldspawn KVP in TrenchBroom.

**Sign convention:** negative = downward, positive = upward. Standard Earth gravity is `-9.81` m/s².

```typescript
// TypeScript
import { world } from "postretro";

const g = world.getGravity();   // → -9.81 at level load (from initialGravity KVP)
world.setGravity(-4.9);         // half gravity — effect is immediate
```

```lua
-- Luau
local g = world:getGravity()   -- → -9.81 at level load
world:setGravity(-4.9)
```

`setGravity` rejects `NaN` and non-finite values silently (a warning is logged) so a misbehaving script cannot break particle physics. The value persists until the next level load or another `setGravity` call.

**TrenchBroom KVP:** set `initialGravity` (float, m/s²) on the `worldspawn` entity. The KVP is required — prl-build errors if absent. Example: `"initialGravity" "-9.81"`.

**Particle effect:** `world.setGravity` directly affects particle buoyancy. Particles with `buoyancy < 0` (heavier-than-air) fall faster under stronger gravity; particles with `buoyancy > 0` (lighter-than-air) float less.

---

## LightAnimation

A `LightAnimation` describes one looping (or finite) animation cycle. All fields except `periodMs` are optional — omit a field to leave that channel unchanged.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `periodMs` | `number` | required | Total duration of one cycle, in milliseconds. |
| `brightness` | `number[]` | `null` | Brightness multiplier samples. The GPU interpolates via Catmull-Rom over the period. `0` = off, `1` = full intensity. Values above `1` are valid. |
| `color` | `Vec3[]` | `null` | RGB color samples (`{ x, y, z }`). **Dynamic lights only.** `setAnimation` throws at the call site if `color` is non-null on a baked light. |
| `direction` | `Vec3[]` | `null` | Unit-vector direction samples for spot lights. Non-unit samples are silently normalized. Zero-length samples are rejected with `InvalidArgument`. |
| `phase` | `number` | `null` | Offset into the cycle where this light starts, in `[0, 1)`. Use to stagger lights in a sequence. Values outside `[0, 1)` are normalized automatically. |
| `playCount` | `number` | `null` | Number of complete cycles to play, then stop. `null` loops forever. |
| `startActive` | `boolean` | `null` (true) | `false` defers the animation until an event activates the entity. Mirrors the FGD `_start_inactive` flag. |

---

## LightComponent

The full component state returned in `LightEntity.component`. All fields are read-only on the snapshot; use `setAnimation` to mutate the live entity.

| Field | Type | Description |
|-------|------|-------------|
| `lightType` | `"Point" \| "Spot" \| "Directional"` | Light shape. |
| `intensity` | `number` | Brightness multiplier (linear, unbounded). |
| `color` | `{ x, y, z }` | Linear RGB base color, nominally `[0, 1]`. |
| `falloffModel` | `"Linear" \| "InverseDistance" \| "InverseSquared"` | Attenuation model. |
| `falloffRange` | `number` | Attenuation radius, in meters. |
| `coneAngleInner` | `number \| null` | Inner cone half-angle in radians. `null` for non-Spot lights. |
| `coneAngleOuter` | `number \| null` | Outer cone half-angle in radians. `null` for non-Spot lights. |
| `coneDirection` | `{ x, y, z } \| null` | Normalized aim vector. `null` for Point lights. |
| `isDynamic` | `boolean` | Same as `LightEntity.isDynamic`. |
| `animation` | `LightAnimation \| null` | Active animation, or `null` if none. Reflects any animation set by `setAnimation` in a previous frame. |

---

## Vocabulary helpers

Import from `"postretro"` (TypeScript) or use the bare globals directly (Luau) — `flicker`, `pulse`, `colorShift`, `sweep`, `timeline`, and `sequence` are installed by the engine prelude; no require or import is needed in Luau. Each helper returns a `LightAnimation` object without touching the engine — pass the result to `setAnimation`. Generic keyframe utilities (`timeline`, `sequence`, the `Keyframe` type) are usable with any keyframed animation, not only lights.

None of the helpers set `phase`. Set it at the call site when staggering multiple lights.

### `flicker`

```typescript
flicker(minBrightness: number, maxBrightness: number, rate: number): LightAnimation
```

Returns an 8-sample irregular brightness curve. `rate` is flicker frequency in Hz; `periodMs` is `1000 / rate`.

```typescript
light.setAnimation(flicker(0.2, 1.0, 8));
```

```lua
light:setAnimation(flicker(0.2, 1.0, 8))
```

---

### `pulse`

```typescript
pulse(minBrightness: number, maxBrightness: number, periodMs: number): LightAnimation
```

Returns a 16-sample sine-approximating brightness curve oscillating between the given bounds over one period.

```typescript
light.setAnimation(pulse(0.4, 1.0, 2000));
```

```lua
light:setAnimation(pulse(0.4, 1.0, 2000))
```

---

### `colorShift`

```typescript
colorShift(colors: [number, number, number][], periodMs: number): LightAnimation
```

Cycles uniformly through the given RGB colors over `periodMs`. Dynamic lights only.

```typescript
light.setAnimation(colorShift([[1, 0, 0], [0, 0, 1]], 3000));
```

---

### `sweep`

```typescript
sweep(directions: [number, number, number][], periodMs: number): LightAnimation
```

Sweeps a spot light's direction through the given unit vectors over `periodMs`. Samples are normalized; zero-length vectors error at the primitive.

```typescript
light.setAnimation(sweep([[1, 0, 0], [0, 0, -1], [-1, 0, 0]], 4000));
```

---

### `timeline`

```typescript
timeline<T extends number[]>(keyframes: [number, ...T][]): [number, ...T][]
```

Validates and returns a list of `[absolute_ms, ...values]` keyframes. Each entry is `[timestamp, ...channelValues]` where timestamps must be strictly increasing. `timeline` does not construct a `LightAnimation` itself — it validates the keyframe shape and returns the array for you to embed in an animation.

Throws a descriptive error if any entry has the wrong arity, a non-finite value, or an out-of-order timestamp.

```typescript
const kf = timeline([
  [   0, 0.0],
  [ 500, 1.0],
  [1000, 0.0],
]);
light.setAnimation({ periodMs: 1000, brightness: kf.map(([, v]) => v) });
```

---

### `sequence`

```typescript
sequence<T extends number[]>(keyframes: [number, ...T][]): [number, ...T][]
```

Same as `timeline`, but accepts `[delta_ms, ...values]` keyframes. The first entry is passed through verbatim; each subsequent timestamp is accumulated from the running sum of deltas. Returns the canonical absolute-timestamp form.

Non-positive deltas produce a non-monotonic timestamp and throw a descriptive error.

```typescript
const kf = sequence([
  [  0, 0.0],  // t = 0 ms
  [200, 1.0],  // t = 200 ms
  [300, 0.5],  // t = 500 ms
  [500, 0.0],  // t = 1000 ms
]);
```

In Luau, arrays are 1-indexed, so keyframe entries are `{timestamp_or_delta, value, ...}` tables:

```lua
local kf = sequence({
  {  0, 0.0 },
  {200, 1.0 },
  {300, 0.5 },
  {500, 0.0 },
})
```

---

## LightEntity handle methods

Methods on the handle returned by `world.query`. In TypeScript, called as `light.method()`; in Luau, called as `light:method()`.

### `setAnimation(anim | null)`

Replaces the current animation. Pass `null` / `nil` to clear it. The last call wins — all lights are interruptible.

```typescript
light.setAnimation(pulse(0.4, 1.0, 2000));
light.setAnimation(null); // clears it
```

---

## FogVolumeComponent

Returned in `FogVolumeHandle.component` from `world.query({ component: "fog_volume" })`. All fields are read-only on the snapshot; mutate the live entity by registering a sequenced reaction whose steps invoke the fog reaction primitives below.

| Field | Type | Description |
|-------|------|-------------|
| `density` | `number` | Optical density of the volume. `0` is transparent; values above `1` saturate quickly. Wire default: `0.5`. |
| `scatter` | `number` | Mie scattering anisotropy in `[0.0, 1.0]`. Higher values bias scattered light forward. Wire default: `0.6`. |
| `edgeSoftness` | `number` | Soft falloff width at the volume boundary, in meters. `0` is a hard edge. |
| `falloff` | `number` | Radial falloff exponent. Used by `fog_lamp`, `fog_tube`, and axis-aligned `fog_volume` (ellipsoid path). Stored on plane-bounded `fog_volume` (non-axis-aligned) entities but not consulted by their shader path. Wire default per FGD: `fog_lamp` = `2.0`, `fog_tube` = `1.5`, axis-aligned `fog_volume` = `2.0`. |
| `tint` | `readonly [number, number, number]` | Per-volume RGB scatter multiplier in linear space. `[1, 1, 1]` = no tint. Each channel clamped to `[0, +∞)`. |
| `saturation` | `number` | Saturation of transmitted SH irradiance: `0` = greyscale, `1` = natural, `>1` = boosted. Default `1.0`. Clamped to `[0, +∞)`. |

---

## Reaction primitives

Reaction primitives are dispatched from sequenced reactions registered via `registerReaction("levelLoad", { sequence: [...] })`. Each step in the sequence carries `{ id, primitive, args }`. The scripting VM is not live at runtime — primitives execute entirely in Rust against the entity registry.

The fog reaction primitives are tag-targeted: when the surrounding reaction's `tag` filter resolves to a list of fog-bearing entities, every match receives the update. Entities matched by tag but lacking a `FogVolumeComponent` are skipped with `log::warn!` (typo guard). Empty target sets are a debug-log no-op.

### `setFogDensity`

```typescript
{ density: number }
```

Overwrites `FogVolumeComponent.density` on every target. `density` must be finite and `>= 0`; out-of-range values clamp to `0.0` with a `log::warn!`. There is no upper clamp — large values saturate the shader.

### `setFogScatter`

```typescript
{ scatter: number }
```

Overwrites `FogVolumeComponent.scatter` on every target. `scatter` must be finite and within `[0.0, 1.0]`; out-of-range values clamp into range with a `log::warn!`.

### `setFogEdgeSoftness`

```typescript
{ edgeSoftness: number }
```

Overwrites `FogVolumeComponent.edgeSoftness` on every target. `edgeSoftness` must be finite and `>= 0`; out-of-range values clamp to `0.0` with a `log::warn!`.

### `setFogFalloff`

```typescript
{ falloff: number }
```

Overwrites `FogVolumeComponent.falloff` on every target. `falloff` must be finite and strictly `> 0`; out-of-range values are dropped (the target's existing `falloff` is preserved) with a `log::warn!`. Accepted on every fog entity type — `fog_volume` plane-sweep volumes store the value but their shader path does not read it.

### `setFogParams`

```typescript
{
  density?: number,
  scatter?: number,
  edgeSoftness?: number,
  falloff?: number,
  tint?: readonly [number, number, number],
  saturation?: number,
}
```

Combined partial-update primitive. Any subset of the six fields may be present. Each field is validated independently per the rules above (out-of-range `density` / `scatter` / `edgeSoftness` / `tint` channel / `saturation` clamp; out-of-range `falloff` is dropped). Absent fields preserve the target's current component value. The component is mutated once per target with the merged result; if all supplied fields fail validation, no write occurs for any target.

Use `setFogParams` when an author wants to change two or more fields atomically — adjacent single-field steps would briefly observe a partial update on the GPU.

### `applyDamage`

```typescript
defineReaction("dummiesCleared", {
  primitive: "applyDamage",
  tag: "player",
  args: { amount: 35 },
});
```

Routes a fixed `amount` of damage through the engine's damage chokepoint for
every entity that matches the reaction's `tag` and carries a health component.
Tag-targeted like the fog primitives: the `tag` resolves to a list of entities
and each match takes the hit. This is the only non-weapon damage producer — use
it to script scene damage (a trap, a collapsing floor, a retaliation strike).

`amount` must be **finite and `>= 0`** (the chokepoint only ever reduces HP;
healing is out of scope). The handler never despawns — a target driven to zero HP
is resolved by the next death sweep, the same path a weapon kill takes.

**This reaction only fires through the death-event drain.** Name the reaction
(the first `defineReaction` argument) to match the event that triggers it. A
`progress` reaction's `fire` event reaches `applyDamage`; the plain movement /
weapon event drains do not, and `levelLoad` fires before the first frame (so a
drop there is invisible). The canonical use is a `progress` threshold that fires
an event of the same name — see [the combat-demo
walkthrough](../content/dev/maps/combat-demo.README.md).

---

## Constraints and errors

| Situation | Result |
|-----------|--------|
| Color animation (`color` field) on a non-dynamic light | Throws at the `setAnimation` call site with a message naming the light's entity id. |
| Zero-length vector in `direction` samples | Rejected by `setLightAnimation` with `InvalidArgument`. |
| Non-unit direction vectors | Silently normalized by the engine. |
| Fog reaction primitive targets a tag with no matching entities | Debug-log no-op. |
| Fog reaction primitive targets an entity lacking `FogVolumeComponent` | Skipped with `log::warn!` (tag-typo guard). |
| `applyDamage` `amount` is negative or non-finite | The whole dispatch is a `log::warn!` no-op — no target takes damage (healing is out of scope). |
| `applyDamage` targets an entity lacking a health component | Skipped with `log::warn!` (tag-typo guard); other matched targets still take damage. |

---

## Player events and slots

### The `playerDied` event

When the player pawn's HP reaches zero, the death sweep fires the `playerDied`
event **exactly once** — it is latched, so a pawn that lingers at zero HP never
re-fires it. Unlike a non-player entity, the player is not despawned by the sweep.
Bind a named reaction to `playerDied` to script the death sequence (a HUD fade, a
respawn prompt, a level restart).

### The readonly `player.health` slot

`player.health` is a readonly, engine-owned HUD store slot. The engine publishes
the live pawn HP into it every frame, and the slot's range is `[0, max]`, where
`max` is the player descriptor's authored `health.max`. A HUD widget binds to it
to draw the health readout; the slot follows automatically as the player takes
damage (e.g. from an `applyDamage` reaction). It is **read-only from scripts** —
the engine is its sole producer, so a script reads it to drive UI but cannot
write it. If the player descriptor declares no `health` block, no HP is published
and the slot keeps its prior range.
