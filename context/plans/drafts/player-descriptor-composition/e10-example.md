# E10 Scripting Surface — Speculative End-State Example

One annotated mod start-script showing the full Epic 10 (Animated Enemies) scripting vocabulary in the composed descriptor style this plan sets. Three provenance tags mark each block:

- **SHIPPED** — today's surface, unchanged.
- **THIS PLAN** — the reshape `index.md` specifies.
- **SPECULATIVE** — direction for future specs (hit zones, AI behavior, wieldable viewFeel overlay). Shapes are vision, not contract; the owning spec pins them.

Navigation has no script surface by design: walkable surfaces bake in prl-build, pathfinding runs engine-side. The script's only touchpoint is the behavior graph's `navigate` intent.

```ts
// Proposed design — start-script.ts
import { defineEntity, runtime } from "postretro";

// ───────────────────────────── Player ─────────────────────────────

const player = defineEntity({
  canonicalName: "player",                         // SHIPPED
  defaultWeapon: "reference_pistol",               // SHIPPED — equip-at-spawn
  components: {
    health: { max: 100 },                          // SHIPPED — feeds player.health slot (E13 contract)

    movement: {                                    // THIS PLAN — composed shape
      capsule: { radius: 0.2, halfHeight: 0.8, eyeHeight: 0.5 },

      states: {
        ground: {
          speed: { walk: 7.0, run: 11.0 },
          accel: 8.0,
          stepHeight: 0.5,
          maxSlope: 45.0,
          // Dash is a child of ground: presence enables it here.
          // Ground dashes keep less momentum — a designed, readable burst.
          dash: { momentumRetention: 0.4 },
        },
        air: {
          forwardSteer: 0.5,
          accel: 10,
          maxControlSpeed: 2,
          bunnyHop: true,
          terminalVelocity: 40.0,
          jump: { velocity: 9, ceiling: 0.0, airCount: 1 },  // one double-jump
          // Air dashes compose momentum more fluidly, and carry a budget.
          dash: { charges: 1, momentumRetention: 0.7, preserveVertical: false },
        },
        crouch: {
          speed: 3.0,                              // moved out of ground.speed
          halfHeight: 0.4,
          eyeHeight: 0.3,
          transitionRate: 8.0,
        },
      },

      modifiers: {
        // Shared dash defaults; the per-state blocks above override sparsely.
        dash: {
          boostSpeed: 22.0,
          // Expression fields keep working at either level:
          steerControl: runtime.clamp(
            runtime.div(runtime.read("elapsedMs"), 150.0), 0.0, 1.0),
          drag: 0,
          cooldownMs: 600,
          preserveVertical: false,
        },
      },

      viewFeel: {                                  // THIS PLAN — layered stack
        base: {
          bob: {
            verticalFrequency: 0.25, lateralFrequency: 0.125,  // half = figure-eight gait
            verticalAmplitude: 0.04, lateralAmplitude: 0.06,
            speedThreshold: 2.0,
          },
          tilt: { speedReference: 10, maxAngle: 4, tension: 15 },
          sway: { amplitude: 0.3, frequency: 0.4, speedScale: 0.02 },
        },
        layers: {
          // Tiers are input-selected (run key), not speed-measured.
          run:    { bob: { verticalAmplitude: 0.07, lateralAmplitude: 0.09 } },
          // Crouch finally owns its own feel — tighter, slower, no tilt.
          crouch: { bob: { verticalFrequency: 0.4, verticalAmplitude: 0.02 }, tilt: null },
          // Replaces the old groundedOnly booleans, explicitly:
          air:    { bob: null, tilt: null },
          // Dash kills sway for the burst so the camera reads clean.
          dash:   { sway: null },
        },
      },

      forgiveness: { coyoteMs: 100, jumpBufferMs: 100 },  // SHIPPED
    },
  },
});

// ───────────────────────────── Weapons ─────────────────────────────

const referencePistol = defineEntity({
  canonicalName: "reference_pistol",               // SHIPPED — E10 weapon primitives
  components: {
    weapon: {
      damage: 12,
      range: 1200,
      fireRateMs: 280,
      fireMode: "semi",
      resolution: "hitscan",
    },
  },
});

const heavyCannon = defineEntity({
  canonicalName: "heavy_cannon",
  components: {
    weapon: {                                      // SHIPPED fields…
      damage: 60,
      range: 2000,
      fireRateMs: 900,
      fireMode: "semi",
      resolution: "hitscan",
    },
    // SPECULATIVE — wieldable viewFeel overlay (lands with wieldable-instance
    // work, research/weapon-model.md). The top layer of the viewFeel stack:
    // applied while this weapon is wielded, sparse-merged over the player's
    // resolved feel. Carrying the big gun reads heavy without the player
    // descriptor knowing this weapon exists.
    viewFeel: {
      bob:  { verticalFrequency: 0.18, verticalAmplitude: 0.06 },  // slower, heavier gait
      tilt: { tension: 8, maxAngle: 2.5 },                          // sluggish strafe response
      sway: { amplitude: 0.6, speedScale: 0.04 },                   // mass drifts the view
    },
  },
});

// ───────────────────────────── Enemy ─────────────────────────────

const grunt = defineEntity({
  canonicalName: "grunt",                          // map-placeable by classname
  components: {
    mesh: {                                        // SHIPPED — skinned animation runtime
      model: "models/grunt.gltf",
      animations: {
        idle:       { clip: "idle",   loop: true },
        locomotion: { clip: "walk",   loop: true,  crossfadeMs: 150 },
        attack:     { clip: "swipe",  crossfadeMs: 80, interrupt: "snap" },
        death:      { clip: "death",  crossfadeMs: 100 },
      },
      defaultState: "idle",
    },

    health: {                                      // SHIPPED — E10 health/damage
      max: 60,
      hitbox: { halfExtents: [0.4, 0.9, 0.4], offset: [0, 0.9, 0] },
    },

    // SPECULATIVE — skeletal hit zones (E10 plan, not yet built). The model
    // ships spatial tags via glTF extras; the script ships the balance.
    // Multipliers scale the incoming DamagePayload per tagged bone capsule.
    hitZones: {
      head: 2.0,
      limb: 0.5,
      default: 1.0,
    },

    // SPECULATIVE — enemy AI behavior (E10 plan, not yet built). Deliberately
    // the SAME {from, to, when} transition-row grammar movement pinned
    // (movement.md §2): closed states, closed predicate vocabulary, data-only.
    // One grammar across subsystems is the composition story.
    behavior: {
      initial: "idle",
      states: {
        idle:   { animation: "idle" },
        alert:  { animation: "locomotion", navigate: "toPlayer" },  // engine pathfinds; script states intent
        attack: { animation: "attack",
                  strike: { damage: 8, rangeM: 2.0, cooldownMs: 1200 } },
        death:  { animation: "death", despawnAfterMs: 2000 },
      },
      transitions: [
        { from: "idle",   to: "alert",  when: { all: ["playerVisible"] } },
        { from: "alert",  to: "attack", when: { all: [{ withinRangeM: 2.0 }] } },
        { from: "attack", to: "alert",  when: { any: [{ beyondRangeM: 3.0 }] } },
        { from: "*",      to: "death",  when: { all: ["died"] } },
      ],
    },
  },
});

export function setupMod() {
  return {
    name: "m10-reference",
    entities: [player, referencePistol, heavyCannon, grunt],
  };
}
```

## What this demonstrates, epic item by item

| E10 item | Where it appears |
|---|---|
| Mesh render path + `MeshComponent` | `grunt.components.mesh.model` |
| Skinned animation runtime (state map) | `mesh.animations` + `defaultState`; behavior states select animation by name |
| Weapon primitives | `referencePistol` / `heavyCannon` weapon blocks; `player.defaultWeapon` |
| Entity health + damage | `health` on grunt (hitscan-targetable) and player (closes the loop both ways) |
| Navigation / pathfinding | no script surface — `navigate: "toPlayer"` is the sole intent hook |
| Skeletal hit zones | `grunt.components.hitZones` (speculative; balance in script, tags in glTF) |
| Enemy AI behavior | `grunt.components.behavior` (speculative; movement's transition grammar reused) |

## The through-line

Every block follows the same composition rules `index.md` sets: closed engine vocabulary, data-only declarations, **shared defaults + sparse per-context overrides** (dash under states, viewFeel layers, weapon overlay), and **one transition grammar** wherever a state machine is authored (movement transitions, AI behavior). A new spec that wants per-context variation should reach for a child block under the contexts it varies across — never a new top-level sibling with a flag.
