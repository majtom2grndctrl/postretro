--!strict
-- The dev mod's hand-authored Luau twin of `mounts-surface.ts`: the
-- `mesh.mounts` Scripting surface example as dev content.
-- Luau names each declaration explicitly.

local M = {}

local AttackTokens = require("./scripts/attack-tokens.luau")

-- The weapons the real dev content defines; the entries name them by value.
local burstRifle = defineEntity({ canonicalName = "enemy_rifle_burst", components = { weapon = {} } })
local longRifle = defineEntity({ canonicalName = "enemy_rifle_long", components = { weapon = {} } })
local armCannon = defineEntity({ canonicalName = "arm_cannon", components = { weapon = {} } })

local aim = defineStance("aim", { animation = "idle_aiming", holdMs = 500 })
local settle = defineStance("settle", { animation = "idle_aiming" })
local burst = defineAttack("burst", {
  draws = { AttackTokens.ranged },
  weapon = "enemy_rifle_burst",
  standoffDistance = 6,
  beginningStance = aim,
  endingStance = settle,
  release = { animation = "shoot", at = false },
})
local longshot = defineAttack("longshot", {
  draws = { AttackTokens.ranged },
  weapon = "enemy_rifle_long",
  standoffDistance = 6,
  beginningStance = aim,
  endingStance = settle,
  release = { animation = "shoot", at = false },
})

-- Guard receivers named once: Luau's method-call sugar would otherwise build
-- each twice.
local targetVisible = brain.targetVisible
local fired = brain.attacksFiredInActivity:ge(1)

M.surfaceLimitatorEntity = defineEntity({
  canonicalName = "surface_limitator",
  components = {
    mesh = {
      model = "models/limitator/model.gltf",
      mounts = {
        { item = burstRifle, attachments = { hand_r = "models/ar_4/limitator.gltf" } },
        { item = longRifle, attachments = { hand_r = "models/ar_4/limitator.gltf" } },
      },
      animations = {
        idle = { clip = "idle", loop = true },
        idle_aiming = { clip = "idle_aiming", loop = true },
        shoot = { clip = "firing_rifle", loop = true, crossfadeMs = 80 },
      },
      defaultState = "idle",
    },
    behavior = {
      initial = "close",
      moveSpeed = 4,
      activities = {
        close = { animation = "idle", motion = "chaseTarget" },
      },
      transitions = {
        close = { { to = aim, when = targetVisible["and"](targetVisible, attackTurnFree(burst)) } },
        aim = { { to = burst }, { to = longshot, when = brain.targetDistance:gt(8) } },
        settle = {
          {
            to = "close",
            when = fired["or"](fired, brain.attacksHeldInActivity:ge(1)),
          },
        },
      },
    },
  },
})

local cannonShot = defineAttack("cannonShot", {
  draws = { AttackTokens.ranged },
  weapon = "arm_cannon",
  standoffDistance = 8,
  beginningStance = aim,
  endingStance = settle,
  release = { animation = "shoot", at = false },
})

M.surfaceCannonBruteEntity = defineEntity({
  canonicalName = "surface_cannon_brute",
  components = {
    mesh = {
      model = "models/mutant_arm_cannon/model.gltf",
      mounts = { { item = armCannon, muzzleSocket = "cannon_tip" } },
      animations = {
        idle = { clip = "idle", loop = true },
        idle_aiming = { clip = "idle", loop = true },
        shoot = { clip = "smash_strike", loop = false, interrupt = "snap" },
      },
      defaultState = "idle",
    },
    behavior = {
      initial = "close",
      moveSpeed = 3,
      activities = {
        close = { animation = "idle", motion = "chaseTarget" },
      },
      transitions = {
        close = { { to = aim, when = brain.targetVisible } },
        aim = { { to = cannonShot } },
        settle = { { to = "close", when = brain.timeInActivityMs:ge(400) } },
      },
    },
  },
})

return M
