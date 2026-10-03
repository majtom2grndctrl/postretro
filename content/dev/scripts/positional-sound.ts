// DEV FIXTURE — positional sound events: the scripting surface of
// `audio.md` §4, shipped so every sound key it names loads and plays in dev
// maps. Hand-authored beside `positional-sound.luau`; the scripting-core twin
// test keeps their descriptors equal.
// Keys under `fixtures/` are generated placeholder tones kept for tests and
// dev play; weapon keys name curated clips.

import { activation, brain, defineEntity, defineReaction } from "postretro";
import type { EmitterParams } from "postretro";
import { playSound } from "postretro/ui";

export const POSITIONAL_SOUND_SHOTGUN = "positional_sound_shotgun";
export const POSITIONAL_SOUND_GRUNT = "positional_sound_grunt";

/** Descriptor path: the engine plays each at the event's anchor, on SFX. */
export const positionalSoundShotgunEntity = defineEntity({
  canonicalName: POSITIONAL_SOUND_SHOTGUN,
  components: {
    weapon: {
      damage: 3.0,
      pelletCount: 8,
      spreadDegrees: 5,
      range: 64.0,
      primary: { trigger: "press", recoveryMs: 700.0, steps: [activation.shot()] },
      resolution: "hitscan",
      resource: {
        kind: "ammo",
        type: "shells.buck",
        magazine: 8,
        reserve: 32,
        reloadMs: 450,
        reloadStyle: "perShell",
      },
      sounds: {
        fire: "weapons/shotgun_fire",
        dryFire: "weapons/dry_fire",
        impact: "fixtures/pellet_hit",
        reloadStart: "weapons/shotgun_pump_back",
        reloadShell: "weapons/shotgun_shell_load", // perShell reload style
        reloadComplete: "weapons/shotgun_pump_forward",
      },
    },
  },
});

/** Landing and jumping, adopted by the dev player's movement block. */
export const positionalSoundMovementSounds = { land: "fixtures/land", jump: "fixtures/jump" };

/** An attack that sounds as it fires, and a state that sounds on entry with no `onEnter`. */
export const positionalSoundGruntEntity = defineEntity({
  canonicalName: POSITIONAL_SOUND_GRUNT,
  components: {
    health: { max: 40, hitbox: { halfExtents: [0.4, 0.9, 0.4], offset: [0, 0.9, 0] } },
    behavior: {
      initial: "idle",
      moveSpeed: 3,
      attacks: { bite: { damage: 10, maxRange: 2, cooldownMs: 800, sound: "fixtures/bite" } },
      activities: {
        idle: { animation: "idle", motion: "hold" },
        alerted: {
          animation: "idle",
          motion: "chaseTarget",
          action: { attack: "bite" },
          sound: "fixtures/growl",
        },
      },
      transitions: { idle: [{ to: "alerted", when: brain.hasTarget }], "*": [] },
    },
  },
});

/** Mod-wide attenuation, adopted by the dev mod's `defineMod`. Every field is optional. */
export const positionalSoundAttenuation = {
  minDistance: 2,
  maxDistance: 60,
  curve: "linear" as const,
};

/** Reaction path: `at` positions a scripted sound at the event's emitter. */
export const positionalSoundReactions = [
  // A dev map door authoring `open_event` "door.open" plays this at its bounds center.
  defineReaction("door.open", (on: EmitterParams) =>
    playSound("fixtures/door_open", { at: on.emitter }),
  ),
  // Unanchored: plays unpositioned.
  defineReaction("positionalSoundAlert", playSound("sfx/test_tone", { bus: "sfx" })),
];
