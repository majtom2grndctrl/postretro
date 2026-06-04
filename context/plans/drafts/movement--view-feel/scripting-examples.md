# movement--view-feel — scripting examples

Proposed API surface for the `viewFeel` sub-descriptor. All examples are
`// Proposed design` — this feature is not yet implemented.

Field names follow the boundary inventory in `index.md`. Wire keys are camelCase
throughout; Rust uses snake_case internally.

---

## TypeScript

### Balanced default player (all three motions)

```typescript
// Proposed design
import { defineEntity } from "postretro";

export const playerEntity = defineEntity({
  canonicalName: "player",
  components: {
    movement: {
      capsule: { radius: 0.2, halfHeight: 0.8, eyeHeight: 0.5 },
      ground: {
        speed: { walk: 7.0, run: 11.0 },
        accel: 8.0,
        stepHeight: 0.5,
        maxSlope: 45.0,
      },
      air: {
        forwardSteer: 0.5,
        accel: 10,
        maxControlSpeed: 2,
        bunnyHop: true,
        jumps: 0,
        jumpVelocity: 9,
        jumpCeiling: 0.0,
      },
      fall: { terminalVelocity: 40.0 },
      viewFeel: {
        bob: {
          frequency: 1.5,          // cycles per metre of horizontal travel
          verticalAmplitude: 0.04, // metres; vertical eye offset at peak
          lateralAmplitude: 0.02,  // metres; side-to-side sway
          speedThreshold: 1.5,     // m/s; below this bob is zero
        },
        tilt: {
          maxAngle: 3.0,           // degrees; max view roll at full strafe speed
          speedReference: 7.0,     // m/s lateral speed that produces maxAngle
          tension: 12.0,           // spring stiffness; higher = snappier settle
        },
        sway: {
          amplitude: 0.3,          // degrees; base wander amplitude
          frequency: 0.4,          // Hz; base oscillation rate
          speedScale: 0.06,        // amplitude gain per m/s of horizontal speed
        },
      },
    },
  },
});
```

---

### Heavy tank (low tension, slow lumbering bob, strong tilt)

Low `tension` makes the tilt spring slow to settle and prone to overshoot —
it reads as mass and inertia. Lower `frequency` with higher amplitudes gives a
slow, weighty footstep cadence.

```typescript
// Proposed design
export const tankEntity = defineEntity({
  canonicalName: "heavy_tank",
  components: {
    movement: {
      capsule: { radius: 0.35, halfHeight: 1.0, eyeHeight: 0.6 },
      ground: {
        speed: { walk: 4.0, run: 6.5 },
        accel: 5.0,
        stepHeight: 0.4,
        maxSlope: 40.0,
      },
      air: {
        forwardSteer: 0.2,
        accel: 4.0,
        maxControlSpeed: 1.0,
        bunnyHop: false,
        jumps: 0,
        jumpVelocity: 6.0,
        jumpCeiling: 0.0,
      },
      fall: { terminalVelocity: 55.0 },
      viewFeel: {
        bob: {
          frequency: 0.9,          // slow cadence — one bob per ~1.1 m
          verticalAmplitude: 0.07, // exaggerated vertical thud
          lateralAmplitude: 0.03,
          speedThreshold: 1.0,
        },
        tilt: {
          maxAngle: 5.0,           // more pronounced lean
          speedReference: 5.0,     // full lean at a slower strafe speed
          tension: 5.0,            // low tension = heavy, slow-to-settle, overshoot
        },
        sway: {
          amplitude: 0.5,
          frequency: 0.25,         // very slow wander
          speedScale: 0.04,
        },
      },
    },
  },
});
```

---

### Nimble scout (high tension, quick bob, minimal sway)

High `tension` snaps the tilt roll to target almost immediately — agile and responsive.
Fast `frequency` with lower amplitude suggests light, quick footsteps.

```typescript
// Proposed design
export const scoutEntity = defineEntity({
  canonicalName: "scout",
  components: {
    movement: {
      capsule: { radius: 0.15, halfHeight: 0.7, eyeHeight: 0.45 },
      ground: {
        speed: { walk: 8.0, run: 14.0 },
        accel: 14.0,
        stepHeight: 0.35,
        maxSlope: 50.0,
      },
      air: {
        forwardSteer: 0.8,
        accel: 16.0,
        maxControlSpeed: 4.0,
        bunnyHop: true,
        jumps: 1,
        jumpVelocity: 11.0,
        jumpCeiling: 0.0,
      },
      fall: { terminalVelocity: 35.0 },
      viewFeel: {
        bob: {
          frequency: 2.2,          // quick cadence — light, energetic
          verticalAmplitude: 0.025,
          lateralAmplitude: 0.01,
          speedThreshold: 2.0,     // higher deadzone — feel kicks in only when running
        },
        tilt: {
          maxAngle: 2.5,
          speedReference: 9.0,
          tension: 24.0,           // high tension = snappy, tracks strafe instantly
        },
        sway: {
          amplitude: 0.15,         // minimal wander — focused, controlled
          frequency: 0.6,
          speedScale: 0.03,
        },
      },
    },
  },
});
```

---

### Alien creature (dominant sway, no tilt)

Omitting `tilt` disables strafe roll entirely. Heavy sway with a strong
`speedScale` gives an unsettling organic lurch at speed — alien gait without
mechanical lean.

```typescript
// Proposed design
export const alienEntity = defineEntity({
  canonicalName: "alien_stalker",
  components: {
    movement: {
      capsule: { radius: 0.25, halfHeight: 0.9, eyeHeight: 0.55 },
      ground: {
        speed: { walk: 5.5, run: 9.0 },
        accel: 7.0,
        stepHeight: 0.45,
        maxSlope: 55.0,
      },
      air: {
        forwardSteer: 0.4,
        accel: 8.0,
        maxControlSpeed: 2.5,
        bunnyHop: false,
        jumps: 0,
        jumpVelocity: 8.0,
        jumpCeiling: 0.0,
      },
      fall: { terminalVelocity: 45.0 },
      viewFeel: {
        bob: {
          frequency: 1.2,
          verticalAmplitude: 0.05,
          lateralAmplitude: 0.04,  // more lateral than vertical — crawling quality
          speedThreshold: 1.0,
        },
        // tilt omitted — no strafe roll for this character
        sway: {
          amplitude: 0.8,          // pronounced ambient wander
          frequency: 0.35,
          speedScale: 0.12,        // sway grows strongly with speed
        },
      },
    },
  },
});
```

---

### No view feel (omit the sub-descriptor entirely)

`viewFeel` is optional. When absent, the view transform is bit-identical to
the pre-view-feel path — no roll, no offset, no overhead.

```typescript
// Proposed design
export const minimalistEntity = defineEntity({
  canonicalName: "turret_camera",
  components: {
    movement: {
      capsule: { radius: 0.1, halfHeight: 0.5, eyeHeight: 0.4 },
      ground: {
        speed: { walk: 0.0, run: 0.0 },
        accel: 0.0,
        stepHeight: 0.0,
        maxSlope: 0.0,
      },
      air: {
        forwardSteer: 0.0,
        accel: 0.0,
        maxControlSpeed: 0.0,
        bunnyHop: false,
        jumps: 0,
        jumpVelocity: 0.0,
        jumpCeiling: 0.0,
      },
      fall: { terminalVelocity: 0.0 },
      // viewFeel absent — camera is perfectly steady
    },
  },
});
```

---

## Luau

The same sub-descriptor works in Luau with identical field names.

### Balanced player

```lua
-- Proposed design
local postretro = require("postretro")

local playerEntity = postretro.defineEntity({
  canonicalName = "player",
  components = {
    movement = {
      capsule = { radius = 0.2, halfHeight = 0.8, eyeHeight = 0.5 },
      ground = {
        speed = { walk = 7.0, run = 11.0 },
        accel = 8.0,
        stepHeight = 0.5,
        maxSlope = 45.0,
      },
      air = {
        forwardSteer = 0.5,
        accel = 10,
        maxControlSpeed = 2,
        bunnyHop = true,
        jumps = 0,
        jumpVelocity = 9,
        jumpCeiling = 0.0,
      },
      fall = { terminalVelocity = 40.0 },
      viewFeel = {
        bob = {
          frequency = 1.5,
          verticalAmplitude = 0.04,
          lateralAmplitude = 0.02,
          speedThreshold = 1.5,
        },
        tilt = {
          maxAngle = 3.0,
          speedReference = 7.0,
          tension = 12.0,
        },
        sway = {
          amplitude = 0.3,
          frequency = 0.4,
          speedScale = 0.06,
        },
      },
    },
  },
})
```

### Heavy tank

```lua
-- Proposed design
local tankEntity = postretro.defineEntity({
  canonicalName = "heavy_tank",
  components = {
    movement = {
      capsule = { radius = 0.35, halfHeight = 1.0, eyeHeight = 0.6 },
      ground = {
        speed = { walk = 4.0, run = 6.5 },
        accel = 5.0,
        stepHeight = 0.4,
        maxSlope = 40.0,
      },
      air = {
        forwardSteer = 0.2,
        accel = 4.0,
        maxControlSpeed = 1.0,
        bunnyHop = false,
        jumps = 0,
        jumpVelocity = 6.0,
        jumpCeiling = 0.0,
      },
      fall = { terminalVelocity = 55.0 },
      viewFeel = {
        bob = {
          frequency = 0.9,
          verticalAmplitude = 0.07,
          lateralAmplitude = 0.03,
          speedThreshold = 1.0,
        },
        tilt = {
          maxAngle = 5.0,
          speedReference = 5.0,
          tension = 5.0,
        },
        sway = {
          amplitude = 0.5,
          frequency = 0.25,
          speedScale = 0.04,
        },
      },
    },
  },
})
```

---

## Field quick-reference

| Field | Unit / range | Effect |
|---|---|---|
| `bob.frequency` | cycles/m, > 0 | Steps per metre of travel; lower = lumbering, higher = quick |
| `bob.verticalAmplitude` | m, ≥ 0 | Eye offset at bob peak; larger = more thud |
| `bob.lateralAmplitude` | m, ≥ 0 | Side-to-side eye drift per bob cycle |
| `bob.speedThreshold` | m/s, ≥ 0 | Minimum speed before bob engages |
| `tilt.maxAngle` | degrees, [0, 90] | Max view roll at full strafe speed |
| `tilt.speedReference` | m/s, > 0 | Lateral speed that produces `maxAngle` |
| `tilt.tension` | > 0 | Spring stiffness: low = heavy/slow-settle, high = snappy |
| `sway.amplitude` | degrees, ≥ 0 | Base wander magnitude at rest |
| `sway.frequency` | Hz, > 0 | Base rate of the noise oscillation |
| `sway.speedScale` | ≥ 0 | Extra amplitude gain per m/s of horizontal speed |

**Two-level optionality.** `viewFeel` itself is optional on `movement`. Within a
present `viewFeel`, each of `bob`, `tilt`, `sway` is independently optional —
omit any to disable that motion. Within a present sub-object, every field is
required.
