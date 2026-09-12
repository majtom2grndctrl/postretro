// ILLUSTRATIVE — proposed syntax, does not compile. See ./README.md.
// Probes: IR-COMPUTED MOTION PARAMS. Today `motion` is a bare verb string and the
// brain scope resolves NO output. A reactive standoff ("kite harder when hurt") needs
// a leaf to resolve motion parameters as IR — the single largest architectural fork.

import { brain, defineEntity, runtime } from "postretro";
import type { EntityTypeDescriptor } from "postretro";

const FIRE_RANGE = 14;
const BREAK_RANGE = 20;
const AIM_MS = 350;
const FIRE_MS = 250;

// Tuning for the reactive standoff.
const BASE_STANDOFF = 6; // holds this far at full health
const KITE_GAIN = 5; // adds up to this much as health drops to zero
const MIN_STANDOFF = 4;
const MAX_STANDOFF = 12;
const ORBIT_SPEED = 1.5; // m/s tangential while engaged

// PROPOSED: standoff = BASE + KITE_GAIN * (1 - health/maxHealth), clamped.
// Built entirely from TODAY's `runtime.*` builders over `brain.*` leaves — the only
// new thing is that a motion leaf is allowed to *carry* this IR as a param.
const reactiveStandoff = runtime.clamp(
  runtime.add(
    BASE_STANDOFF,
    runtime.mul(
      KITE_GAIN,
      runtime.sub(1, runtime.div(brain.health, brain.maxHealth)),
    ),
  ),
  MIN_STANDOFF,
  MAX_STANDOFF,
);

export const straferEntity: EntityTypeDescriptor = defineEntity({
  canonicalName: "strafer",
  components: {
    health: { max: 40 },
    mesh: { model: "strafer" },
    behavior: {
      initial: "idle",
      moveSpeed: 4,
      engagementRadius: FIRE_RANGE,
      attacks: {
        shoot: { weapon: "enemy_rifle", standoffDistance: BASE_STANDOFF }, // TODAY: fixed number
      },
      activities: {
        idle: { animation: "idle", motion: "hold" },
        engage: {
          animation: "run",
          layers: {
            // The movement leaf. TODAY this is `move: [ ..rows, "chaseTarget" ]`.
            move: [
              // Option A — VERB-ADD: keep the verb set, let a motion leaf be an object
              // with a verb + IR params + explicit anchor. `strafe` is a PROPOSED verb.
              {
                when: brain.targetVisible,
                motion: {
                  verb: "strafe", // PROPOSED verb: hold standoff + orbit the anchor
                  anchor: "target", // PROPOSED: was implicit in `chaseTarget`
                  standoff: reactiveStandoff, // PROPOSED: IR param (the fork)
                  orbit: ORBIT_SPEED, // PROPOSED: tangential speed; sign could itself be IR
                },
              },
              "chaseTarget", // TODAY fallback: close when we can't see the target
            ],

            // Option B — VERB-COLLAPSE: one `positionGoal` motion subsumes chaseTarget /
            // moveToAnchor / patrol. Anchor + offset + engaged are all params.
            // move: [
            //   {
            //     when: brain.targetVisible,
            //     motion: {
            //       verb: "positionGoal", // PROPOSED single verb
            //       anchor: "target",     // "target" | "anchor" | "lastKnown" | <entity>
            //       standoff: reactiveStandoff,
            //       bias: ORBIT_SPEED,    // lateral/tangential
            //       engaged: true,        // retain the combat slot + allow action
            //     },
            //   },
            //   "chaseTarget",
            // ],

            offense: {
              initial: "aim",
              activities: {
                aim: { animation: "idle_aiming" },
                fire: { animation: "shoot", action: { attack: "shoot" } },
              },
              transitions: {
                aim: [
                  { to: "fire", when: brain.timeInActivityMs.ge(AIM_MS) },
                  { to: "aim", when: brain.targetVisible.not() },
                ],
                fire: [{ to: "aim", when: brain.timeInActivityMs.ge(FIRE_MS) }],
              },
            },
          },
        },
      },
      transitions: {
        idle: [
          { to: "engage", when: brain.acquisitionDue.and(brain.targetDistance.le(FIRE_RANGE)) },
        ],
        engage: [{ to: "idle", when: brain.targetDistance.gt(BREAK_RANGE) }],
      },
    },
  },
});

// Open question this file makes concrete: does `standoff:`/`orbit:` accepting a RuntimeValue
// mean the *brain scope grows a write/resolve path*, or do motion params bind in a *separate
// motion-parameter scope* (like the movement-local scope) that reads brain facts but resolves
// its own outputs? The guards above stay read-only either way.
