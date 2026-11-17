// DEMO CONTENT — `limitator` descriptor.
//
// A ranged-combat enemy on the Mixamo skeleton (converted from Limitator.fbx via
// mixamo_to_gltf.py). Locomotion / idle / aim / death clips come from the Mixamo
// "Pro Rifle Pack"; the attack (`firing_rifle`) plus `reloading` and
// `hit_reaction` are merged in from the Mixamo "Basic Shooter Pack" — the Pro
// Rifle Pack has no firing clip. The model is baked facing engine +Z (yaw 90)
// so the AI facing (MESH_FORWARD = +Z) turns his front toward the target.
//
// The AR_4 rifle (Quaternius "Modular Sci Fi Guns", CC0) mounts on the `hand_r`
// socket that mixamo_to_gltf.py tagged on the skeleton, with its grip-relative
// orientation re-tuned for this skeleton bake so the barrel points forward.
//
// In a fight it takes turns with its squad on the `ranged` token. Running a
// far target down, it stops now and then to sight the rifle and fire one
// careful long shot, then runs on. Within strafing range it panics: it strafes
// around its target, throws the rifle up when a turn is free and sprays a
// three-shot burst, then runs to a flanking slot. Each reposition's roll picks
// the next cycle: strafe and spray again, or stop and aim, a longer stance
// whose release is the same three-shot burst. Enough damage during the far
// sight or the aim staggers it.
//
// Map-placeable via `"classname" "limitator"`.

import { attackTurnFree, brain, defineAttack, defineEntity, defineStance } from "postretro";
import { ranged } from "./attack-tokens";

// Ranged-combat tuning. Unlike the melee `reference_enemy`, the limitator wants
// to hold a firing standoff rather than close to contact.
const DETECTION_RANGE = 50;
// Firing threshold: at or inside this distance, with a clear sightline, the
// enemy stops running the target down and starts its panicked strafe-and-spray
// cycle.
// Kept well below DETECTION_RANGE so there is a wide band (FIRE..DETECTION) in
// which he actively runs the target down — otherwise, engaging already-in-range,
// he never visibly chases.
const FIRE_RANGE = 8;
// Break-off distance (hysteresis): once firing, only re-close after the target
// pulls past this. The FIRE..BREAK band keeps a target dancing on the boundary
// from endlessly restarting the cycle.
const BREAK_RANGE = 9;
// The combat-slot standoff the squad strafes, fires and repositions at — held
// strictly INSIDE FIRE_RANGE.
const STANDOFF_DISTANCE = 6;
// Leash: abandon the chase and walk home once dragged this far from the spawn
// anchor, then stand down. Set above DETECTION_RANGE so a target acquired near
// the detection edge isn't leashed the instant he starts closing.
const LEASH_RANGE = 70;
const RETURN_ARRIVAL_EPSILON = 1;

// Every shot leaves on `firing_rifle`'s `kick`, the loop's recoil at its
// start, so a strike's first shot waits for the loop's next kick from where
// the clip is drawn: up to one 267 ms pass after its pose releases.
// No reload beat — the available reload clip
// is a crouching animation that swings the right hand through a wide arc, which
// tips the rigidly-mounted rifle upside-down and into the chest. A reload that
// reads correctly needs a clip that keeps the gun hand steady, or a weapon
// animated as a skinned part rather than a rigid socket attachment.

// The far long shot. Running the target down beyond FIRE_RANGE, with it in
// sight within LONG_RANGE and a `ranged` turn free, he stops and sights the
// rifle (`sight`, shouldered on `idle_aiming`) for a hidden time drawn from
// SIGHT_HOLD_MS, then fires one `enemy_rifle_long` shot, which reaches 20 m.
// Only after RUN_MS in `close` does he stop: `close` restarts on every entry,
// so he visibly runs before each stop, the first included. That is his
// pacing, not the shot's cooldown, which is the weapon's 750 ms recovery and
// long spent by the time RUN_MS has run again. A stop comes about every 4 s:
// the sight, up to a pass waiting for the kick, the shot's recover, the
// settle, then RUN_MS of running.
const LONG_RANGE = 20;
const RUN_MS = 2000;
const SIGHT_HOLD_MS: [number, number] = [1000, 1333];
// A sight kept waiting in line this long gives up and runs on.
const SIGHT_WAIT_MS = 2000;
// The long shot's release leaves the tick after its shot; its recover plays
// the recoil out on the firing loop, ending short of the loop's next kick
// 267 ms on, so no second recoil plays without a shot.
const LONG_SHOT_RECOVER_MS = 200;

// The panicked spray: strafe at least WAIT_MS, then, when a `ranged` turn is
// free, throw the rifle up (`panic`) for exactly PANIC_HOLD_MS and fire
// `enemy_rifle_burst`'s three shots. Then settle and reposition to a slot
// 60-120 degrees around the target from where it fired, for MOVE_MS.
// The burst rifle's `recoveryMs`, restarted by each shot, and the span of its
// three shots on whole 60 Hz ticks: the rifle is ready that long after a
// burst's first shot. The aim's burst fires the same rifle. Every route into
// `panic` runs through `wait`, entered after any earlier burst's first shot,
// so WAIT_MS alone covers both: the spray never waits in line on a cooling
// rifle.
const RIFLE_RECOVERY_MS = 1000;
const RIFLE_BURST_SPAN_MS = 267;
const WAIT_MS = RIFLE_RECOVERY_MS + RIFLE_BURST_SPAN_MS;
const PANIC_HOLD_MS = 500;
// A panic kept waiting in line this long gives up and strafes on.
const PANIC_WAIT_MS = 1000;
const MOVE_MS = 1500;
// Each reposition rolls once on entry: below this share it strafes and sprays
// next, else it stops and aims.
const STRAFE_SHARE = 0.5;
// The aim: the rifle shouldered on `idle_aiming` for a hidden time drawn from
// AIM_HOLD_MS (the pose's whole time, its wait in line included), at or above
// the co-op floor, then the burst. Enough damage while it aims, or sights
// from afar, breaks it into a flinch; an aim kept waiting in line gives up
// and strafes.
const AIM_HOLD_MS: [number, number] = [500, 900];
const AIM_BREAK_DAMAGE = 25;
const AIM_WAIT_MS = 2000;
// A burst, the aim's or the panicked spray, leaves its release the tick after
// its first shot, on the rifle's `kick`; its recover keeps the turn and the
// firing loop through the other two shots, 16 ticks after the first, with two
// ticks to spare.
const BURST_RECOVER_MS = 300;
// After each aimed strike the rifle stays up a beat before he moves on.
const SETTLE_MS = 250;
// `hit_reaction` plays 433 ms; the flinch holds its last frame briefly.
const FLINCH_MS = 500;

// Stop and aim: the shouldered rifle names the threat, not its timing.
const aim = defineStance({
  animation: "idle_aiming",
  holdMs: AIM_HOLD_MS,
  breakOn: { damage: AIM_BREAK_DAMAGE, to: "flinch" },
});
// The strafe's panicked aim, held a fixed half second.
const panic = defineStance({ animation: "idle_aiming", holdMs: PANIC_HOLD_MS });
// The aim's burst and the spray land here, not back in their tells, whose
// rests would strike again.
const settle = defineStance({ animation: "idle_aiming" });
// The far aim, taken from the run.
const sight = defineStance({
  animation: "idle_aiming",
  holdMs: SIGHT_HOLD_MS,
  breakOn: { damage: AIM_BREAK_DAMAGE, to: "flinch" },
});
// The long shot lands here, then runs on.
const steady = defineStance({ animation: "idle_aiming" });

// Each weapon owns damage, reach, and cooldown; these attacks keep only AI
// positioning and their draw. Every release fires on `firing_rifle`'s `kick`.
// The burst rifle's 12 m reach covers the fire band (entered inside
// FIRE_RANGE and held to BREAK_RANGE) with a small safety margin.
const shoot = defineAttack({
  draws: [ranged],
  weapon: "enemy_rifle_burst",
  standoffDistance: STANDOFF_DISTANCE,
  beginningStance: aim,
  endingStance: settle,
  release: { animation: "shoot", at: "kick" },
  recover: { animation: "shoot", ms: BURST_RECOVER_MS },
});
const spray = defineAttack({
  draws: [ranged],
  weapon: "enemy_rifle_burst",
  standoffDistance: STANDOFF_DISTANCE,
  beginningStance: panic,
  endingStance: settle,
  release: { animation: "shoot", at: "kick" },
  recover: { animation: "shoot", ms: BURST_RECOVER_MS },
});
// The long rifle reaches LONG_RANGE, so a target that held still through the
// sight is in reach at the kick; one that stepped past it is missed.
const longshot = defineAttack({
  draws: [ranged],
  weapon: "enemy_rifle_long",
  standoffDistance: STANDOFF_DISTANCE,
  beginningStance: sight,
  endingStance: steady,
  release: { animation: "shoot", at: "kick" },
  recover: { animation: "shoot", ms: LONG_SHOT_RECOVER_MS },
});

export const limitatorEntity = defineEntity({
  canonicalName: "limitator",
  components: {
    health: {
      max: 100,
      // ~1.82 m tall (model baked at scale 0.68); hitbox spans feet to head,
      // centered on the torso. The head primitive tops out at y≈1.82, so the
      // box half-height/offset are 0.91 (not the body primitive's 0.81 max) —
      // otherwise upper-head shots above y≈1.62 miss the health hitbox.
      hitbox: {
        halfExtents: [0.27, 0.91, 0.27],
        offset: [0, 0.91, 0],
      },
      zoneMultipliers: {
        head: 2.5,
        leg: 0.5,
      },
    },
    // One ordinary hit is enough to make this ranged fixture reconsider its target.
    tolerance: 4,
    mesh: {
      model: "models/limitator/model.gltf",
      attachments: {
        hand_r: "models/ar_4/model.gltf",
      },
      animations: {
        idle: { clip: "idle", loop: true },
        idle_aiming: { clip: "idle_aiming", loop: true },
        // The strafe's walk. Facing its target, the engine plays the
        // directional state matching its travel; each clip's stride matches
        // the forward walk's.
        walk: {
          clip: "walk_forward",
          loop: true,
          crossfadeMs: 200,
          travelSpeed: 1.5,
        },
        walk_left: {
          clip: "walk_left",
          loop: true,
          crossfadeMs: 200,
          travelSpeed: 1.5,
        },
        walk_right: {
          clip: "walk_right",
          loop: true,
          crossfadeMs: 200,
          travelSpeed: 1.5,
        },
        walk_backward: {
          clip: "walk_backward",
          loop: true,
          crossfadeMs: 200,
          travelSpeed: 1.5,
        },
        run: {
          clip: "run_forward",
          loop: true,
          crossfadeMs: 200,
          travelSpeed: 4,
        },
        // Attack pose (Basic Shooter Pack): a fast recoil kick, looped through
        // each burst and the long shot's recover. Its `kick` event marks the
        // recoil, at the clip's start.
        shoot: {
          clip: "firing_rifle",
          loop: true,
          crossfadeMs: 80,
        },
        // Reload clip available but NOT wired: it is a crouching reload that
        // swings the gun hand through a wide arc, tipping the rigidly-mounted AR
        // upside-down and into the chest. Needs a steady-hand reload clip (or a
        // skinned weapon) before it can drive a reload beat.
        reload: { clip: "reloading", loop: false, crossfadeMs: 150 },
        // A sight's or aim's break plays this from the graph's `flinch` activity,
        // which leaves on its own timer, or on any outer row (target lost),
        // whose activity sets the next state. It is never an impact-policy
        // `playAnim`: that only switches the state and never recovers, so it
        // froze on its last frame once the enemy went idle.
        hit_reaction: {
          clip: "hit_reaction",
          loop: false,
          crossfadeMs: 80,
          interrupt: "snap",
        },
        death: {
          clip: "death_from_front_headshot",
          loop: false,
          crossfadeMs: 120,
          interrupt: "snap",
        },
      },
      defaultState: "idle",
      // Scale the locomotion clip, the strafe's walk and its directional
      // variants, to actual XZ speed so the stride reads at the agent's real
      // pace; the run's authored stride already matches `moveSpeed`. Required
      // alongside `behavior` to materialize the nav agent that grounds and
      // moves the entity.
      locomotion: { speedScale: true },
    },
    // The behavior graph is what materializes the engine brain AND the nav
    // agent; without it the entity has no floor grounding and never ticks.
    behavior: {
      initial: "idle",
      moveSpeed: 4,
      // The strafe ring and reposition slots sit at this radius, the shoot
      // standoff, so the squad circles at its firing distance.
      engagementRadius: STANDOFF_DISTANCE,
      activities: {
        idle: { animation: "idle", motion: "hold" },
        // The composite's `walk` names the graph's locomotion clip, so the
        // strafe's walk and its directional variants scale to its speed.
        engage: {
          animation: "walk",
          layers: {
            // Only `close` takes its motion from here: run the target down
            // until it is inside FIRE_RANGE with a clear sightline. Every
            // stance, the far sight taken from `close` included, stands still.
            move: [
              {
                when: brain.targetDistance
                  .le(FIRE_RANGE)
                  .and(brain.targetVisible),
                motion: "hold",
              },
              "chaseTarget",
            ],
            offense: {
              initial: "close",
              activities: {
                close: { animation: "run" },
                // Strafe around the target, facing it, while a squadmate shoots.
                wait: { animation: "walk", motion: "strafe" },
                // Fire, then relocate: a slot to the side of where it fired.
                // Its roll picks the next cycle.
                move: {
                  animation: "run",
                  motion: "reposition",
                  facing: "travel",
                  reposition: { arc: [60, 120] },
                },
                // The break of a sight or an aim.
                flinch: { animation: "hit_reaction", motion: "hold" },
              },
              transitions: {
                // In range, the strafe. Short of it, after RUN_MS of
                // running, the far sight; its range rows keep a target inside
                // FIRE_RANGE from ever starting it.
                close: [
                  {
                    to: "wait",
                    when: brain.targetDistance
                      .le(FIRE_RANGE)
                      .and(brain.targetVisible),
                  },
                  {
                    to: sight,
                    when: brain.timeInActivityMs
                      .ge(RUN_MS)
                      .and(brain.targetVisible)
                      .and(brain.targetDistance.gt(FIRE_RANGE))
                      .and(brain.targetDistance.le(LONG_RANGE))
                      .and(attackTurnFree(longshot)),
                  },
                ],
                // Losing the sightline or the range returns to `close`, after
                // the LOS grace window; distance hysteresis keeps a target on
                // the boundary from churning.
                wait: [
                  { to: "close", when: brain.targetVisible.not() },
                  { to: "close", when: brain.targetDistance.gt(BREAK_RANGE) },
                  {
                    to: panic,
                    when: brain.timeInActivityMs
                      .ge(WAIT_MS)
                      .and(attackTurnFree(spray)),
                  },
                ],
                // Rows out of the panic's rest. Once its turn is granted the
                // hold releases as the pose reaches PANIC_HOLD_MS, and the
                // burst follows at the next kick.
                panic: [
                  { to: "close", when: brain.targetVisible.not() },
                  { to: "close", when: brain.targetDistance.gt(BREAK_RANGE) },
                  { to: spray },
                  { to: "wait", when: brain.timeInActivityMs.ge(PANIC_WAIT_MS) },
                ],
                // Rows out of the sight's rest, where he waits in line: a
                // target lost, past the long shot's reach, or come inside
                // FIRE_RANGE sends him back to `close`, whose first row then
                // starts the strafe. Once the turn is granted the hold has no
                // range rows, so a target that walks inside FIRE_RANGE
                // mid-sight still takes the long shot, which reaches it;
                // `steady` then hands him to `close` and the strafe.
                sight: [
                  { to: "close", when: brain.targetVisible.not() },
                  { to: "close", when: brain.targetDistance.gt(LONG_RANGE) },
                  { to: "close", when: brain.targetDistance.le(FIRE_RANGE) },
                  { to: longshot },
                  { to: "close", when: brain.timeInActivityMs.ge(SIGHT_WAIT_MS) },
                ],
                steady: [{ to: "close", when: brain.timeInActivityMs.ge(SETTLE_MS) }],
                move: [
                  {
                    to: "wait",
                    when: brain.timeInActivityMs
                      .ge(MOVE_MS)
                      .and(brain.activityRoll.lt(STRAFE_SHARE)),
                  },
                  { to: aim, when: brain.timeInActivityMs.ge(MOVE_MS) },
                ],
                // Rows out of the aim's rest. The row to `shoot` also waits
                // for its `ranged` turn and the rifle's recovery.
                aim: [
                  { to: "close", when: brain.targetVisible.not() },
                  { to: "close", when: brain.targetDistance.gt(BREAK_RANGE) },
                  { to: shoot },
                  { to: "wait", when: brain.timeInActivityMs.ge(AIM_WAIT_MS) },
                ],
                settle: [{ to: "move", when: brain.timeInActivityMs.ge(SETTLE_MS) }],
                // A flinch past BREAK_RANGE, a far sight's, runs the target
                // down again; one in range repositions.
                flinch: [
                  {
                    to: "close",
                    when: brain.timeInActivityMs
                      .ge(FLINCH_MS)
                      .and(brain.targetDistance.gt(BREAK_RANGE)),
                  },
                  { to: "move", when: brain.timeInActivityMs.ge(FLINCH_MS) },
                ],
              },
            },
          },
        },
        // Leashed: walk back to the spawn anchor after being dragged too far.
        retreat: { animation: "run", motion: "moveToAnchor" },
      },
      transitions: {
        "*": [
          { to: "idle", when: brain.hasTarget.not() },
          // A selected neutral target is an engine-admitted over-tolerance
          // retaliator, so it remains engageable like an ordinary hostile.
        ],
        idle: [
          {
            to: "engage",
            when: brain.acquisitionDue.and(
              brain.targetDistance.le(DETECTION_RANGE),
            ),
          },
        ],
        // Give up the chase once dragged past the leash; stand down at home.
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
