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
| `tag` | `string \| null` | The entity's tag at query time. |
| `component` | `LightComponent` | Full component snapshot at query time. See [LightComponent](#lightcomponent) below. |

#### Example — rolling wave down a hallway

Tag the hallway lights `"hallway_wave"` in TrenchBroom. The script queries them at level load, sorts along the x axis, and staggers `phase` so the pulse travels.

**TypeScript**

```typescript
import { registerHandler } from "postretro";
import { world } from "./sdk/world";
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
local world = require("sdk/world")

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

Import from `sdk/light_animation.ts` (TypeScript) or `require("sdk/light_animation")` (Luau). Each helper returns a `LightAnimation` object without touching the engine — pass the result to `setAnimation`.

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
light:setAnimation(LightAnimationSdk.flicker(0.2, 1.0, 8))
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
light:setAnimation(LightAnimationSdk.pulse(0.4, 1.0, 2000))
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
local kf = LightAnimationSdk.sequence({
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
| Zero-length vector in `direction` samples | Rejected by `set_light_animation` with `InvalidArgument`. |
| Non-unit direction vectors | Silently normalized by the engine. |
| Calling `world.query` outside a `registerHandler` callback | Error — behavior context only. |

---

## Complete example

### TypeScript

Drop this into `content/base/scripts/hallway_wave.ts`. Tag the hallway lights `"hallway_wave"` in TrenchBroom and one additional light `"boss_light"` somewhere in the map.

```typescript
import { registerHandler } from "postretro";
import { world } from "./sdk/world";
import { pulse, flicker } from "./sdk/light_animation";
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
local world = require("sdk/world")
local LightAnimationSdk = require("sdk/light_animation")

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
    local anim = LightAnimationSdk.flicker(0.1, 0.9, 12)
    anim.phase = 0.3
    boss:setAnimation(anim)
  end
end)
```
