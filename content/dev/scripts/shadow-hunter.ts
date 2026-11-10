// DEMO CONTENT — `shadow_hunter` descriptor.
//
// A fast melee skirmisher, the brute's opposite: the low-poly Decraniated body
// on the Mixamo Mutant creature-pack clips (`mixamo_mutant`). It snarls when it
// spots a target, sprints in, claws quickly and often, and dies fast. A first
// pass for tuning: every number below is meant to be played with.
//
// The body is re-rested onto the library's T-pose and carries no hit zones, so
// hits land on the authored hitbox (see the model's license.txt).
//
// Map-placeable via `"classname" "shadow_hunter"`.

import { brain, defineEntity } from "postretro";

const DETECTION_RANGE = 30;
// The snarl before the sprint: the start of the roar, cut short.
const ALERT_MS = 500;
const CLAW_RANGE = 1.8;
// A claw plays the punch, whose strike lands about 250 ms in; the hunter
// stays planted that long before it can claw or chase again.
const CLAW_MS = 450;
const LEASH_RANGE = 80;
const RETURN_ARRIVAL_EPSILON = 1;

export const SHADOW_HUNTER_CLASSNAME = "shadow_hunter";

export const shadowHunterEntity = defineEntity({
  canonicalName: SHADOW_HUNTER_CLASSNAME,
  components: {
    health: {
      // Fragile: it survives on speed, not armor.
      max: 45,
      // About 1.95 m tall and narrow.
      hitbox: {
        halfExtents: [0.35, 0.97, 0.35],
        offset: [0, 0.97, 0],
      },
    },
    tolerance: 4,
    mesh: {
      model: "models/decraniated_low_poly_retro_pixel/model.gltf",
      animations: {
        idle: { clip: "idle_c", loop: true },
        // Its legs are about 1.2x the Mutant's, so the run's planted foot
        // covers about 2.6 m/s; at `moveSpeed` 5 the clip plays near 1.9x,
        // which is the hunter's scramble.
        run: { clip: "run", loop: true, crossfadeMs: 120, travelSpeed: 2.6 },
        snarl: { clip: "roar_b", loop: false, crossfadeMs: 100, interrupt: "snap" },
        attack_claw: {
          clip: "punch",
          loop: false,
          crossfadeMs: 60,
          interrupt: "snap",
        },
        flinch: {
          clip: "hit_reaction",
          loop: false,
          crossfadeMs: 60,
          interrupt: "snap",
        },
        death: {
          clip: "death_b",
          loop: false,
          crossfadeMs: 100,
          interrupt: "snap",
        },
      },
      defaultState: "idle",
      locomotion: { speedScale: true },
    },
    behavior: {
      initial: "idle",
      moveSpeed: 5,
      attacks: {
        claw: { damage: 6, maxRange: CLAW_RANGE, cooldownMs: 600 },
      },
      engagementRadius: CLAW_RANGE,
      activities: {
        idle: { animation: "idle", motion: "hold" },
        alert: { animation: "snarl", motion: "hold" },
        engage: {
          animation: "run",
          layers: {
            move: [
              { when: brain.targetDistance.le(CLAW_RANGE), motion: "hold" },
              "chaseTarget",
            ],
            offense: {
              initial: "approach",
              activities: {
                approach: { animation: "run" },
                claw: { animation: "attack_claw", action: { attack: "claw" } },
              },
              transitions: {
                approach: [
                  { to: "claw", when: brain.targetDistance.le(CLAW_RANGE) },
                ],
                claw: [
                  { to: "approach", when: brain.timeInActivityMs.ge(CLAW_MS) },
                ],
              },
            },
          },
        },
        retreat: { animation: "run", motion: "moveToAnchor" },
      },
      transitions: {
        "*": [{ to: "idle", when: brain.hasTarget.not() }],
        idle: [
          {
            to: "alert",
            when: brain.acquisitionDue.and(
              brain.targetDistance.le(DETECTION_RANGE),
            ),
          },
        ],
        alert: [{ to: "engage", when: brain.timeInActivityMs.ge(ALERT_MS) }],
        engage: [
          { to: "retreat", when: brain.distanceFromAnchor.gt(LEASH_RANGE) },
        ],
        retreat: [
          { to: "idle", when: brain.distanceFromAnchor.le(RETURN_ARRIVAL_EPSILON) },
        ],
      },
    },
  },
});
