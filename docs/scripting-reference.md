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

## Runtime values

Most descriptor fields are plain literals — you write a number, the engine reads
it once. A handful of fields accept something richer: a **`RuntimeValue`**, a
small expression the engine re-evaluates from live gameplay state.

**The one thing to internalize:** *your script runs once, at load. A `RuntimeValue`
crosses into the engine as data and is re-evaluated from live gameplay state.* You
never write a per-frame callback — there is no live VM during gameplay. Instead you
*describe* a computation with the `runtime.*` builders; that description becomes
engine-owned data, and the engine evaluates it for you at the moment the field
needs a value. A `momentumRetention` that branches on whether you're grounded, a
`steerControl` that ramps up over the course of a dash — both are authored as data,
evaluated by the engine, with no code of yours running at tick time.

### The `runtime.*` builders

`runtime` is a prelude global (like `world`). Each builder returns a plain
`RuntimeValue` node; nest them to compose an expression. The leaves are
`runtime.read(name)` (a live input, bound by name) and `runtime.constant(value)` (a
fixed literal).

```typescript
import { runtime } from "postretro";

// 0.4 while grounded, 0.7 while airborne.
runtime.select(runtime.read("grounded"), 0.4, 0.7);
```

| Builder | Result | Meaning |
|---------|--------|---------|
| `runtime.read(name)` | input leaf | Reads a live value by name (see the table below). |
| `runtime.constant(value)` | literal leaf | A fixed `number` or `boolean`. |
| `runtime.add` / `sub` / `mul` / `div` `(a, b)` | number | Arithmetic. |
| `runtime.clamp(x, lo, hi)` | number | Clamp `x` into `[lo, hi]`. |
| `runtime.lerp(a, b, t)` | number | Linear interpolation between `a` and `b` by `t`. |
| `runtime.lt` / `le` / `gt` / `ge` / `eq` / `ne` `(a, b)` | boolean | Comparisons. |
| `runtime.select(cond, a, b)` | number or boolean | Branchless `cond ? a : b`. `a` and `b` share a type. |

**Literal sugar.** Every builder argument also accepts a bare `number` or
`boolean` — it is auto-wrapped into a `constant` node for you. The two lines below
build identical IR:

```typescript
runtime.add(runtime.read("speed"), runtime.constant(1.0));
runtime.add(runtime.read("speed"), 1.0); // bare literal auto-wraps
```

A bare literal in the *field itself* is the same sugar: `boostSpeed: 22.0` is just
`boostSpeed: runtime.constant(22.0)`. Leave a field literal when it never needs to
vary; reach for `runtime.*` only when the value depends on live state.

### Where runtime values are accepted: dash fields

Today the runtime-value-capable fields all live on `components.movement.dash`. Each
of the five scalar fields accepts `number | RuntimeValue`; `preserveVertical`
accepts `boolean | RuntimeValue`. `airDashes` stays a plain integer (it is a
structural budget, not a derived value).

```typescript
defineEntity({
  canonicalName: "player",
  components: {
    movement: {
      // ...capsule / ground / air / fall...
      dash: {
        boostSpeed: 22.0,
        // Entry-moment: keep less ground momentum than air momentum.
        momentumRetention: runtime.select(runtime.read("grounded"), 0.4, 0.7),
        // Per-tick: steering authority ramps 0 → 1 over the first 150 ms.
        steerControl: runtime.clamp(
          runtime.div(runtime.read("elapsedMs"), 150.0),
          0.0,
          1.0,
        ),
        dashDrag: 0,
        cooldownMs: 600,
        airDashes: 1,
        preserveVertical: false,
      },
    },
  },
});
```

### `read` names available to dash fields

A dash expression binds against a fixed, read-only **movement** namespace — these
six names and no others. Reading any other name is a load-time error (see below).
There is no access to the state store from here; a dash field reads movement state
only.

| `read` name | Type | Meaning |
|-------------|------|---------|
| `speed` | `number` | Horizontal speed, `\|velocity.xz\|`, world-units/sec. |
| `verticalSpeed` | `number` | Vertical velocity (`velocity.y`); positive is up. |
| `grounded` | `boolean` | Whether the pawn is on the ground this tick. |
| `chargesRemaining` | `number` | Air dashes left. At dash entry this reads the **post-spend** count — the charges you have *after* committing this dash. |
| `cooldownMs` | `number` | Remaining dash cooldown, in milliseconds. |
| `elapsedMs` | `number` | Milliseconds elapsed in the **current** dash. `0` at entry and outside a dash; it accumulates each tick while dashing. |

### When each field is evaluated

The evaluation moment is engine-pinned per field — you don't choose it. This is why
`elapsedMs` is meaningful for some fields and always `0` for others.

| Field | Evaluated | Useful inputs |
|-------|-----------|---------------|
| `boostSpeed` | **at dash entry**, once | `speed`, `verticalSpeed`, `grounded`, `chargesRemaining` |
| `momentumRetention` | **at dash entry**, once | `speed`, `grounded`, `chargesRemaining` |
| `cooldownMs` | **at dash entry**, once | `chargesRemaining`, `grounded` |
| `preserveVertical` | **at dash entry**, once | `verticalSpeed`, `grounded` |
| `steerControl` | **every tick** while dashing | `elapsedMs`, `speed` |
| `dashDrag` | **every tick** while dashing | `elapsedMs`, `speed` |

Entry-moment fields see `elapsedMs == 0`; only the two per-tick fields see it climb.
Keep any ramp over `elapsedMs` inside the dash's lifetime — the `Dash` state is hard-
bounded at **200 ms** (`DASH_MAX_MS`), so a ramp that completes inside ~150 ms stays
fully observable.

### Ranges still apply

A `RuntimeValue` cannot be range-checked at load (its value isn't known until it
evaluates), so the engine **clamps the evaluated result** to the same range the
literal form enforces — silently, every evaluation: `boostSpeed`, `dashDrag`,
`cooldownMs` clamp to `>= 0`; `momentumRetention`, `steerControl` clamp to
`[0, 1]`. So `momentumRetention` evaluating to `3.0` behaves as `1.0`, and a
`cooldownMs` that goes negative arms as `0`. (One asymmetry: a *literal* `boostSpeed`
of `0` is rejected at load — boost must be positive — but an *expression* that
evaluates to `0` is allowed and yields a zero-boost dash.)

### Validation errors

An expression is type-checked and name-resolved **at load**, the same place every
other malformed descriptor field is caught. A descriptor that loads cannot fail at
tick time. Each row below rejects the descriptor with a descriptive
`InvalidShape` error:

| Situation | Result |
|-----------|--------|
| `runtime.read("notAName")` — a name outside the six movement inputs | Rejected at load: the name does not resolve in the movement scope. |
| Type-table violation — e.g. a boolean operand where a number is required (`runtime.clamp(runtime.read("grounded"), 0, 1)`) | Rejected at load: the operand type does not match the op. |
| Root-type mismatch — a boolean-rooted expression in a number field (or vice versa), e.g. `boostSpeed: runtime.gt(runtime.read("speed"), 5)` | Rejected at load: the expression's result type does not match the field. |
| Malformed node — an object that isn't a recognizable `runtime.*` node | Rejected at load as an invalid expression shape. |
| Literal out of range — a bare-literal field outside its declared bounds (e.g. literal `boostSpeed: 0`) | Rejected at load, exactly as before (unchanged by runtime values). |

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

## System reactions

System reactions are the HUD-dynamics half of the reaction surface (M13 Goal E).
Unlike the tag-targeted primitives above, they carry **no `tag`**: they touch no
entities. Instead each enqueues a typed engine command — audio, force feedback, a
screen flash, or a UI-stack push/pop. The SDK exposes them as pure body builders
that pair with `defineReaction`; the builder returns a `PrimitiveReactionDescriptor`
and has no FFI side effect (the boundary is the `return`). Optional arguments are
omitted from the emitted `args` entirely when not supplied — they are never sent as
`undefined`/`nil`.

| Helper | Emitted body | Notes |
|--------|--------------|-------|
| `playSound(sound, bus?)` | `{ primitive: "playSound", args: { sound, bus? } }` | Routes to the M12 audio module on the optional named mixer `bus` (engine default bus when omitted). |
| `rumble(strong, durationMs, weak?)` | `{ primitive: "rumble", args: { strong, weak?, durationMs } }` | Drives gilrs gamepad force feedback. `strong`/optional `weak` are 0–1 motor intensities; `durationMs` is the rumble length. Warn-once no-op when force feedback is unsupported. |
| `flashScreen(color, durationMs)` | `{ primitive: "flashScreen", args: { color, durationMs } }` | Writes the engine-owned `screen.flash` RGBA slot, which decays back to transparent. `color` is `[r, g, b, a]` (0–1); `durationMs` is the decay time. |
| `showDialog(tree, onCommit?)` | `{ primitive: "showDialog", args: { tree, onCommit? } }` | Pushes the dialog UI `tree` onto the modal stack; optional `onCommit` names a reaction fired on commit. |
| `openTextEntry(onCommit?)` | `{ primitive: "showDialog", args: { tree: "keyboard", onCommit? } }` | Opens the engine-shipped on-screen keyboard (a capturing modal editing `ui.textEntry`). A `showDialog` wrapper targeting the `keyboard` tree. See the text-entry walkthrough below. |
| `openMenu(tree)` | `{ primitive: "openMenu", args: { tree } }` | A v1 alias of `showDialog` (identical push behavior) without the `onCommit` hook. |
| `closeDialog()` | `{ primitive: "closeDialog", args: {} }` | Pops the top UI tree off the modal stack. |
| `appendText(slot, text)` | `{ primitive: "appendText", args: { slot, text } }` | Appends `text` to the current string value of the writable String slot `slot`. Readonly-gated like `setState`. |
| `backspaceText(slot)` | `{ primitive: "backspaceText", args: { slot } }` | Removes the last grapheme cluster (char-pop floor — never splits a UTF-8 sequence) from `slot`. Empty is a silent no-op. Readonly-gated like `setState`. |
| `clearText(slot)` | `{ primitive: "clearText", args: { slot } }` | Empties the writable String slot `slot`. Readonly-gated like `setState`. |

The three UI-stack helpers (`showDialog` / `openMenu` / `closeDialog`) are v1
placeholders: `showDialog` and `openMenu` perform the identical `PushTree`
operation (only `showDialog` carries the optional `onCommit`), and `closeDialog`
pops. Until Goal F's modal stack lands they **warn once ("no stack") and no-op**.

### Firing system reactions on a state crossing

`onStateCrossing(slot, condition, fire)` is the watcher that drives system
reactions from live state. It is a pure builder — place its result in
`setupLevel`'s returned `crossings` array. The engine watches `slot` after each
frame's slot writes and, on a crossing in the condition's direction (from
at-or-past the threshold to across it), fires every named reaction in `fire`
exactly once; it re-arms only after a crossing back. A registration against a
non-Number slot warns and is skipped at load.

The condition is `{ below: number, max?: number }` or `{ above: number, max?: number }`;
`max` is the denominator the threshold is a fraction of (`threshold / max` vs
`value / max`), defaulting to `1.0` for a raw comparison.

The canonical HUD-dynamics pattern — flash red when health drops below 20%:

```typescript
export function setupLevel(): LevelManifest {
  return {
    reactions: [
      defineReaction("lowHealth", flashScreen([1, 0, 0, 0.5], 250)),
    ],
    crossings: [
      // health is 0–100; cross below 20% of `max` fires "lowHealth" once.
      onStateCrossing("player.health", { below: 20, max: 100 }, ["lowHealth"]),
    ],
  };
}
```

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

## Operable UI (M13 Goal F)

Goal F makes the UI operable: a closed nav-intent vocabulary, focusable
interactive widgets, a slot-write reaction, and an engine-owned interaction-mode
slot. The whole surface is keyboard-, mouse-, and gamepad-interchangeable — the
same widget reacts to a gamepad confirm, an Enter key, or a mouse click.

### Nav intents

Navigation reads a **fixed** input vocabulary (not the remappable action table).
Each intent has a stable `nav.*` wire name UI authors reference in `capturesNav`
and focus policy. The `NavIntent` type (template-literal in TS, string union in
Luau) constrains those strings so a typo is a compile error.

| Intent | Wire name | Keyboard | Gamepad |
|--------|-----------|----------|---------|
| Up / Down / Left / Right | `nav.up` … `nav.right` | Arrow keys | D-pad / left stick edge |
| Next / Prev | `nav.next` / `nav.prev` | Tab | Right / Left shoulder |
| Confirm | `nav.confirm` | Enter | A / South |
| Cancel | `nav.cancel` | Escape *(inside a capturing tree)* | B / East |
| Menu | `nav.menu` | Escape *(from gameplay)* | Start |
| Options | `nav.options` | — | Select / Back |

Escape is context-sensitive: from gameplay it is `nav.menu` (opens a menu); inside
a capturing UI tree it is `nav.cancel` (backs out). The left stick produces one
directional intent per push past the dead zone (a flick to the opposite direction
re-fires); holding a direction repeats on a delay→interval timer, not per frame.

### Focus and repeat props

Focusable widgets (`button`, `slider`) form a focus ring the player moves with
directional nav. Directional nav resolves geometrically against the laid-out
rects; authored `focusNeighbors` (a `{ "nav.up": "<id>", … }` map) override the
geometric pick per direction. A tree's `initialFocus` names the node focus starts
on when the tree becomes the top of the modal stack; `restoreOnReturn` on a
container restores its last-focused child when focus returns to it. Held
directional nav repeats on a delay-then-interval timer (the engine's hold-to-
repeat clock), so a held stick or arrow steps focus/value steadily.

### Interactive widgets

- **`button`** — `{ kind: "button", id, label, onPress, focusNeighbors? }`.
  Focusable. Activation (a focus-engine confirm **or** a pointer click) fires the
  `onPress` **named reaction** through the same reaction registry entity/system
  reactions use, so a click and a gamepad confirm have an identical effect. `id`
  is required (activation resolves the focused node id back to `onPress`).
- **`slider`** — `{ kind: "slider", id, label, bind, min, max, step, capturesNav?, focusNeighbors? }`.
  Focusable. `capturesNav` is an **array** of nav wire names (e.g.
  `["nav.left", "nav.right"]`, not a bool) the slider claims first refusal on:
  a captured directional nav steps the bound value by `step` within `[min, max]`
  and emits a `setState` write to the bound slot (applied on the next frame).
  `bind` is `{ slot, tween? }`.
- **`bar`** — `{ kind: "bar", bind, max, fill, background, id?, styleRanges? }`.
  Passive (not focusable). Renders a `background` quad with a `fill` quad whose
  width is `value/max` clamped to `[0, 1]`. `styleRanges` (Goal E) recolors the
  fill band by `value/max`. Horizontal only in v1.

### `setState`

`setState(slot, value)` is a system reaction that writes a value to a **writable**
store slot. It is **readonly-gated**: a write to a readonly slot (e.g. the
engine-owned `player.health`, `input.mode`) logs a warning and no-ops; an
engine-owned but writable slot, or any mod-declared writable slot, is a valid
target. The value is coerced to the slot's declared type (number / boolean /
string / number array) with the same range/enum validation a script store write
applies. This is the path a `slider`'s nav-capture step takes to publish its new
value.

### Text-edit reactions and the `ui.textEntry` slot

`appendText(slot, text)`, `backspaceText(slot)`, and `clearText(slot)` are system
reactions that edit the current **string** value of a **writable** store slot at
the game-logic stage. They share `setState`'s **readonly gate**: a write to a
readonly slot logs a warning and no-ops. `backspaceText` removes one extended
grapheme cluster with a char-pop floor (it pops one Unicode scalar value, so it
never splits a UTF-8 sequence); an empty value is a **silent no-op** (no warning,
no write).

`ui.textEntry` is the engine-declared, **writable** String slot these reactions
target by default — the shared text-edit surface both the hardware-keyboard path
and the on-screen-keyboard asset drive. It defaults to an empty string and is a
valid `setState`/text-edit target (unlike the readonly engine slots).

### Text entry end-to-end (the on-screen keyboard)

Text entry is the gamepad accessibility accommodation: the engine ships an
on-screen keyboard (a capturing modal, registered under the name `keyboard`),
built entirely from `button`/`grid`/`focus: "spatial"` primitives plus the
text-edit reactions above. A player types either on the **hardware keyboard**
(routed straight into `ui.textEntry`) or on the **on-screen keyboard** via gamepad
— both edit the same `ui.textEntry` slot, so a field bound to it reflects either
path.

`openTextEntry(onCommit?)` is the canonical opener — it wraps
`showDialog("keyboard", onCommit)`. Wire it to a `button`'s `onPress`. The
keyboard is a capturing modal: while open, gameplay input freezes and the opener
screen's focus restores on close.

- The keyboard's letter / digit / space keys fire `appendText("ui.textEntry", …)`
  named reactions; its backspace key fires `backspaceText("ui.textEntry")` and
  opts into `repeatOnHold` (holding it repeats; holding a letter fires once).
- The keyboard's **`done`** key and the **hardware Enter** key both **commit**:
  the engine fires the opener's `onCommit` reaction, then closes the keyboard.
- **`nav.cancel`** (Escape / gamepad B) closes the keyboard **without** firing
  `onCommit` — the edits stay in `ui.textEntry`; the opener simply does not act on
  them.

A bound `text` widget reads the live entry directly (no copy); fire an observable
reaction (a `playSound`) from `onCommit` so commit and cancel are distinguishable.

```typescript
export function setupLevel(): LevelManifest {
  return {
    reactions: [
      // The button that opens the keyboard, carrying a commit reaction.
      defineReaction("openName", openTextEntry("onNameEntered")),
      // The observable confirmation fired on commit (done / Enter), not on cancel.
      defineReaction("onNameEntered", playSound("sfx/confirm", "sfx")),
    ],
  };
}
```

Author the screen with a `text` bound to `ui.textEntry` and a button firing the
opener:

```jsonc
{ "kind": "text", "content": "NAME --", "fontSize": 28,
  "color": "ok", "bind": { "slot": "ui.textEntry", "format": "NAME {}" } },
{ "kind": "button", "id": "enterName", "label": "ENTER NAME", "onPress": "openName" }
```

The keyboard layout itself is an engine-shipped JSON asset at
`content/base/ui/keyboard.json`, loaded from disk at boot. Editing it (adding or
removing keys, retiming the backspace repeat) and reloading changes the keyboard
with no engine change — keys are data. Each key's `onPress` names a reaction the
mod registers (the `appendText` / `backspaceText` reactions above), except the
`done` key, whose reserved `onPress` (`ui.commitTextEntry`) the engine intercepts
to reach the shared commit seam.

> **Keyboard asset is layout-only.** `content/base/ui/keyboard.json` ships the key grid but no reactions — it is inert until a mod declares the matching named `appendText` / `backspaceText` reactions each key's `onPress` references (see `content/dev/scripts/arena-lights.ts` for the registration loop).

### The readonly `input.mode` slot

`input.mode` is a readonly, engine-owned enum slot (`"pointer"` | `"focus"`)
reporting the current pointer-vs-focus interaction mode. The engine writes it from
App-side input observation: mouse motion switches it to `"pointer"`, while any
nav input (stick / D-pad / nav key) switches it to `"focus"` (debounced so jitter
doesn't flap it). While a capturing UI tree is on the stack the mode also drives
the OS cursor (visible in `pointer`, hidden in `focus`) and the focus ring (hidden
in `pointer`, visible in `focus`); it is inert when no capturing tree is up. A
`text` widget can `bind` it to display the live mode. It is **read-only from
scripts** — the engine is its sole producer.
