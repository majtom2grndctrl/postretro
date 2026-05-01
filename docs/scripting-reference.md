# Scripting Reference

## Overview

Behavior scripts are TypeScript (`.ts`) or Luau (`.lua` / `.luau`) files placed in `content/<mod>/scripts/`. The engine loads all scripts automatically when a level starts — no registration file needed. While the engine is running, changing a script file triggers a hot-reload: the new code takes effect on the next event dispatch without restarting the level.

Scripts respond to engine events via `registerHandler`. They can query light entities, attach animations, and drive transitions. The GPU evaluates animation curves each frame; scripts only need to set up the animation once.

---

## Events

`registerHandler(event, fn)` is the entry point for all script logic.

| Event | Context parameter | When it fires |
|-------|-------------------|---------------|
| `"levelLoad"` | none | Once when the level starts. Use this to set up light animations. |
| `"tick"` | `{ delta: number, time: number }` | Once per frame. `delta` is seconds since the last frame; `time` is seconds since level load. |

Lighting animations do not need `"tick"` — the GPU evaluates the curves each frame without per-frame script involvement. Use `"tick"` only when you need to compute something that cannot be expressed as a pre-built curve.

**TypeScript**

```typescript
import { registerHandler } from "postretro";

registerHandler("levelLoad", () => {
  // runs once at level start
});

registerHandler("tick", (ctx) => {
  const elapsed = ctx!.time;
  // runs every frame
});
```

**Luau**

```lua
registerHandler("levelLoad", function()
  -- runs once at level start
end)

registerHandler("tick", function(ctx)
  local elapsed = ctx.time
  -- runs every frame
end)
```

---

## world.query

`world.query(filter)` returns an array of entity handles matching a filter. The concrete handle type depends on the `component` you query — currently only `"light"` is supported. Querying an unknown component name throws `InvalidArgument`.

```typescript
world.query({ component: "light" })            // all lights → LightEntity[]
world.query({ component: "light", tag: "foo" }) // only lights tagged "foo"
```

Providing a `tag` narrows the result to entities whose tag matches exactly. `world.query` is only valid inside a `registerHandler` callback (behavior context). Calling it outside that context is an error.

### LightEntity

Returned when `component` is `"light"`. All fields are a snapshot at query time. `setAnimation`, `setIntensity`, and `setColor` operate on the **live** entity by id and do not require a fresh `world.query`.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `EntityId` | Stable entity id. Pass to `set_light_animation` and other primitives. |
| `transform.position` | `{ x, y, z }` | Light origin in world space at query time. |
| `isDynamic` | `boolean` | Whether the light is runtime-dynamic. Sourced from the `_dynamic` key in the `.map` file. Dynamic lights participate in the per-fragment GPU light loop and the shadow-slot scheduler; use this to gate color animations (color animation is only valid on dynamic lights). |
| `tags` | `string[]` | The entity's tags at query time. Empty array if untagged. |
| `component` | `LightComponent` | Full component snapshot at query time. See [LightComponent](#lightcomponent) below. |

#### Example — rolling wave down a hallway

Tag the hallway lights `"hallway_wave"` in TrenchBroom. The script queries them at level load, sorts along the x axis, and staggers `phase` so the pulse travels.

**TypeScript**

```typescript
import { registerHandler, world } from "postretro";
import type { LightAnimation } from "postretro";

registerHandler("levelLoad", () => {
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
});
```

**Luau**

```lua
-- `world` is a bare global installed by the engine prelude — no require needed.
registerHandler("levelLoad", function()
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
end)
```

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

The full component state returned in `LightEntity.component`. All fields are read-only on the snapshot; use `setAnimation`, `setIntensity`, or `setColor` to mutate the live entity.

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
| `castShadows` | `boolean` | Whether the light casts shadows. |
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

### `setIntensity(target, transitionMs?, easing?)`

Transitions the light's intensity to `target` over `transitionMs` milliseconds. `transitionMs` defaults to `0` (instant). `easing` defaults to `"easeInOut"` when `transitionMs > 0`; ignored for instant transitions.

Reads the **live** intensity from the registry at call time (not the query-time snapshot), so chained transitions compose correctly. Internally constructs a one-cycle `LightAnimation` (`playCount: 1`).

```typescript
light.setIntensity(0.0, 1500, "easeOut"); // fade to black over 1.5 s
```

Available easing values: `"linear"`, `"easeIn"`, `"easeOut"`, `"easeInOut"`.

### `setColor(target, transitionMs?, easing?)`

Transitions the light's color to `target` (`[r, g, b]` in TypeScript, `{r, g, b}` array in Luau) over `transitionMs` milliseconds. Same live-read / one-cycle pattern as `setIntensity`. **Dynamic lights only** — throws on baked lights.

```typescript
light.setColor([1, 0.3, 0], 800); // shift to orange over 800 ms
```

---

## Constraints and errors

| Situation | Result |
|-----------|--------|
| Color animation (`color` field or `setColor`) on a non-dynamic light | Throws at the `setAnimation` / `setColor` call site with a message naming the light's entity id. |
| Zero-length vector in `direction` samples | Rejected by `setLightAnimation` with `InvalidArgument`. |
| Non-unit direction vectors | Silently normalized by the engine. |
| Calling `world.query` outside a `registerHandler` callback | Error — behavior context only. |

---

## Complete example

### TypeScript

Drop this into `content/base/scripts/hallway_wave.ts`. Tag the hallway lights `"hallway_wave"` in TrenchBroom and one additional light `"boss_light"` somewhere in the map.

```typescript
import { registerHandler, world, pulse, flicker } from "postretro";
import type { LightAnimation } from "postretro";

registerHandler("levelLoad", () => {
  // Rolling brightness wave across the hallway
  const hallway = world
    .query({ component: "light", tag: "hallway_wave" })
    .sort((a, b) => a.transform.position.x - b.transform.position.x);

  const wave: LightAnimation = {
    periodMs: 10000,
    brightness: [
      0.1, 0.1, 0.1, 0.1, 0.1,
      0.3, 0.8, 1.0, 0.8, 0.3,
      0.1, 0.1, 0.1, 0.1, 0.1,
      0.1, 0.1, 0.1, 0.1, 0.1,
    ],
  };

  hallway.forEach((light, i) => {
    light.setAnimation({ ...wave, phase: i / hallway.length });
  });

  // A single flickering boss-room light with a staggered start
  const [boss] = world.query({ component: "light", tag: "boss_light" });
  if (boss) {
    boss.setAnimation({ ...flicker(0.1, 0.9, 12), phase: 0.3 });
  }
});
```

### Luau

```lua
-- `world`, `flicker`, etc. are bare globals installed by the engine prelude — no require needed.
registerHandler("levelLoad", function()
  -- Rolling brightness wave across the hallway
  local hallway = world:query({ component = "light", tag = "hallway_wave" })
  table.sort(hallway, function(a, b)
    return a.transform.position.x < b.transform.position.x
  end)

  local wave = {
    periodMs = 10000,
    brightness = {
      0.1, 0.1, 0.1, 0.1, 0.1,
      0.3, 0.8, 1.0, 0.8, 0.3,
      0.1, 0.1, 0.1, 0.1, 0.1,
      0.1, 0.1, 0.1, 0.1, 0.1,
    },
  }

  for i, light in ipairs(hallway) do
    light:setAnimation({
      periodMs = wave.periodMs,
      brightness = wave.brightness,
      phase = (i - 1) / #hallway,
    })
  end

  -- A single flickering boss-room light with a staggered start
  local bossLights = world:query({ component = "light", tag = "boss_light" })
  if #bossLights > 0 then
    local boss = bossLights[1]
    local anim = flicker(0.1, 0.9, 12)
    anim.phase = 0.3
    boss:setAnimation(anim)
  end
end)
```

---

## Entity lifecycle

### `spawnEntity(transform, tags?)`

Spawns a new entity at the given transform. `tags` is an optional array of tag strings attached at creation time. Returns an `EntityId`.

```typescript
const id = spawnEntity(
  {
    position: { x: 0, y: 0, z: 64 },
    rotation: { pitch: 0, yaw: 0, roll: 0 },
    scale: { x: 1, y: 1, z: 1 },
  },
  ["myTag"]
);
```

### `despawnEntity(id)`

Removes an entity from the world. The entity stops responding to queries immediately; deferred cleanup runs at end-of-tick.

```typescript
despawnEntity(id);
```

### `entityExists(id)`

Returns `true` if the entity is alive, `false` if it has been despawned or was never valid.

```typescript
if (entityExists(id)) { /* safe to use */ }
```

---

## Query and component access

### `worldQuery(filter)`

Returns an array of entity handles matching the filter. Valid `filter` shapes:

| Filter | Returns | Notes |
|--------|---------|-------|
| `{ component: "transform" }` | `{ id, position, tags }[]` | Returns **every** live entity. Filter by `tag` when you need a subset. The returned handle exposes only `id` (`EntityId`), `position` (`{ x, y, z }` in world space at query time), and `tags` (`string[]`). There is no `rotation` or `scale` field on the handle — use `getComponent(id, "transform")` to read the full `Transform` if you need those. |
| `{ component: "light" }` | `{ id, position, tags, isDynamic, component: LightComponent }[]` | |
| `{ component: "emitter" }` | `{ id, position, tags, component: BillboardEmitterComponent }[]` | |
| `{ component: "particle" }` | `[]` | Always empty — particles are not individually observable by scripts. |
| `{ component: "sprite_visual" }` | `[]` | Always empty — internal rendering detail. |
| `{ tag: "someTag" }` | filtered subset | Narrows any of the above by tag (exact match). |

Unknown `component` strings throw a `ScriptError`.

> **Note:** `worldQuery({ component: "transform" })` returns every live entity with no cap. Use `{ tag: "..." }` to retrieve a targeted subset.

```typescript
const all   = worldQuery({ component: "transform" });
const emitters = worldQuery({ component: "emitter", tag: "campfire" });
```

### `getComponent(id, kind)`

Returns the current component value for the given entity and component kind string (`"light"`, `"billboard_emitter"`, etc.). Throws a `ScriptError` if the entity does not carry that component — use `entityExists` and `worldQuery` to check presence before calling.

```typescript
const emitter = getComponent(id, "billboard_emitter");
```

### `setComponent(id, kind, value)`

Writes a component value onto an entity. The `value` must match the component kind's shape. Changes take effect at the next tick.

> **Note:** `"light"`, `"billboard_emitter"`, `"particle_state"`, and `"sprite_visual"` are read-only via `setComponent`. Use dedicated primitives (`setLightAnimation`, reaction primitives) to mutate those components. Only `"transform"` writes are supported.

```typescript
setComponent(id, "transform", { kind: "transform", position: { x: 0, y: 0, z: 0 }, rotation: { pitch: 0, yaw: 0, roll: 0 }, scale: { x: 1, y: 1, z: 1 } });
```

### `getEntityProperty(id, key)`

Reads a per-placement key-value pair authored on the `.map` entity that spawned this entity. Returns the string value for `key`, or `null` if the key was not set. Available on entities spawned via `registerEntity` archetypes and on built-in classname entities (e.g. `billboard_emitter`).

```typescript
const label = getEntityProperty(id, "display_name"); // null if unset
```

---

## Events

### `emitEvent(event)`

Emits a game event. The event is appended to the `game_events` ring buffer (capacity 1024; oldest entry is evicted when full). The engine drains the buffer at the end of the Game logic phase and logs each entry at `game_events=info`. The event is also broadcast to any script-side handler registered for `kind` via `registerHandler`. Safe to call for event kinds with no registered handler — the call completes cleanly.

```typescript
emitEvent({ kind: "damage", payload: { source: id, amount: 10 } });
```

### `sendEvent(targetId, event)`

Sends an event directly to a specific entity. The entity must be alive; calling with a dead id is a no-op.

```typescript
sendEvent(targetId, { kind: "activate", payload: {} });
```

### `registerHandler(kind, fn)`

Registers a callback for an event kind. Multiple handlers for the same kind all fire. Valid event kinds:

| Kind | Context parameter | When it fires |
|------|-------------------|---------------|
| `"levelLoad"` | none | Once when the level starts. |
| `"tick"` | `{ delta: number, time: number }` | Once per frame. `delta` is seconds since the last tick; `time` is seconds since level load. |

```typescript
registerHandler("tick", (ctx) => {
  const dt = ctx!.delta;
});
```

---

## Data context

### `registerEntity(descriptor)`

Registers a script-defined entity archetype for use across all levels. Call this from a data script (mod scope), before level load.

| Field | Type | Description |
|-------|------|-------------|
| `classname` | `string` | The `.map` classname this archetype matches. Must not conflict with a built-in classname (e.g. `billboard_emitter`) — built-ins take precedence and a warning is logged. |
| `components.emitter` | `ComponentValue` (optional) | Emitter component attached at spawn. Use `smokeEmitter`, `sparkEmitter`, or `emitter()`. |
| `components.light` | `{ color: [r, g, b], range: number, intensity: number, is_dynamic: boolean }` (optional) | Light component attached at spawn. Descriptor-spawned lights are always treated as dynamic regardless of `is_dynamic`. |

**Idempotency:** calling `registerEntity` again with the same classname and descriptor is a silent no-op. If the descriptor differs, the new one wins and a debug log is emitted.

**Archetype spawn order:** after built-in classname dispatch runs at level load, the engine sweeps `world.map_entities` a second time and spawns script-registered archetypes for any entity whose classname matched a `registerEntity` call and was not handled as a built-in.

**KVP overrides with `initial_` prefix:** any `initial_`-prefixed key on a `.map` placement (e.g. `initial_rate`, `initial_range`, `initial_is_dynamic`) overrides the matching descriptor field at spawn time. On parse failure the descriptor default is kept and a warning is logged. The key is `initial_` followed by the descriptor's field name (e.g. `initial_range` overrides `LightDescriptor.range`).

> **Naming note:** `BillboardEmitterComponent.initial_velocity` already starts with `initial_`, so the mechanical override key would be `initial_initial_velocity` (prefix doubled). Both `initial_initial_velocity` and the friendlier alias `initial_velocity` are accepted; either writes to `BillboardEmitterComponent.initial_velocity` at spawn. The shortest alias `velocity` is also accepted and writes the same field.

**KVP read access:** `getEntityProperty` (see above) is available on entities spawned this way, as well as on entities spawned by built-in classname handlers.

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

## Entity API examples

### Data script — registering archetypes

```typescript
// content/mymod/scripts/entities.ts
// Runs at mod init (before level load). No import needed — registerEntity,
// smokeEmitter, and sparkEmitter are engine globals.

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

### Behavior script — query, get, set, and emit

```typescript
// content/mymod/scripts/smoke_manager.ts
import { registerHandler } from "postretro";

registerHandler("levelLoad", () => {
  // Find all exhaust emitters and boost their rate
  const ports = worldQuery({ component: "emitter", tag: "boost" });
  for (const port of ports) {
    // getComponent is safe here — worldQuery already guarantees the emitter component is present.
    const comp = getComponent(port.id, "emitter");
    setComponent(port.id, "emitter", { ...comp, rate: comp.rate * 2 });
  }
});

registerHandler("tick", (ctx) => {
  // Every 5 seconds, emit a gameplay event
  if (Math.floor(ctx!.time) % 5 === 0 && Math.floor(ctx!.time - ctx!.delta) % 5 !== 0) {
    emitEvent("ambientPulse", { time: ctx!.time });
  }
});
```

### Level script — levelLoad and tick handlers

```typescript
// content/mymod/scripts/level_01.ts
import { registerHandler } from "postretro";

registerHandler("levelLoad", () => {
  const campfires = worldQuery({ component: "light", tag: "campfire" });
  for (const fire of campfires) {
    fire.setAnimation(flicker(0.6, 1.0, 6));
  }
});

registerHandler("tick", (ctx) => {
  const elapsed = ctx!.time;
  // Tick-driven logic here — prefer pre-built animations over per-tick mutation.
});
```
