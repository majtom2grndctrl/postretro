--!strict
-- DEV FIXTURE — Luau twin of `positional-sound.ts`: the positional sound
-- events scripting surface of `audio.md` §4. The scripting-core twin test
-- keeps both authorings' descriptors equal.
-- Keys under `fixtures/` are generated placeholder tones kept for tests and
-- dev play; weapon keys name curated clips.

local UI = require("postretro/ui")

local M = {}

M.POSITIONAL_SOUND_SHOTGUN = "positional_sound_shotgun"
M.POSITIONAL_SOUND_GRUNT = "positional_sound_grunt"

-- Descriptor path: the engine plays each at the event's anchor, on SFX.
M.positionalSoundShotgunEntity = defineEntity({
  canonicalName = M.POSITIONAL_SOUND_SHOTGUN,
  components = {
    weapon = {
      damage = 3.0,
      pelletCount = 8,
      spreadDegrees = 5,
      range = 64.0,
      fireRateMs = 700.0,
      fireMode = "semi",
      resolution = "hitscan",
      resource = {
        kind = "ammo",
        type = "shells.buck",
        magazine = 8,
        reserve = 32,
        reloadMs = 450,
        reloadStyle = "perShell",
      },
      sounds = {
        fire = "weapons/shotgun_fire",
        dryFire = "weapons/dry_fire",
        impact = "fixtures/pellet_hit",
        reloadStart = "weapons/shotgun_pump_back",
        reloadShell = "weapons/shotgun_shell_load", -- perShell reload style
        reloadComplete = "weapons/shotgun_pump_forward",
      },
    },
  },
})

-- Landing and jumping, adopted by the dev player's movement block.
M.positionalSoundMovementSounds = { land = "fixtures/land", jump = "fixtures/jump" }

-- An attack that sounds as it fires, and a state that sounds on entry with no `onEnter`.
M.positionalSoundGruntEntity = defineEntity({
  canonicalName = M.POSITIONAL_SOUND_GRUNT,
  components = {
    health = { max = 40, hitbox = { halfExtents = { 0.4, 0.9, 0.4 }, offset = { 0, 0.9, 0 } } },
    behavior = {
      initial = "idle",
      moveSpeed = 3,
      attacks = { bite = { damage = 10, maxRange = 2, cooldownMs = 800, sound = "fixtures/bite" } },
      activities = {
        idle = { animation = "idle", motion = "hold" },
        alerted = {
          animation = "idle",
          motion = "chaseTarget",
          action = { attack = "bite" },
          sound = "fixtures/growl",
        },
      },
      transitions = { idle = { { to = "alerted", when = brain.hasTarget } }, ["*"] = {} },
    },
  },
})

-- Mod-wide attenuation. Every field is optional.
M.positionalSoundAttenuation = { minDistance = 2, maxDistance = 60, curve = "linear" }

-- Reaction path: `at` positions a scripted sound at the event's emitter.
M.positionalSoundReactions = {
  -- A dev map door authoring `open_event` "door.open" plays this at its bounds center.
  defineReaction("door.open", function(on: EmitterParams)
    return UI.playSound("fixtures/door_open", { at = on.emitter })
  end),
  -- Unanchored: plays unpositioned.
  defineReaction("positionalSoundAlert", UI.playSound("sfx/test_tone", { bus = "sfx" })),
}

return M
