--!strict
-- Luau mirror of the dev mod's E16 descriptors: the shielded player, the
-- armored reference enemy's health, the shielded variant and the triangle
-- guns. Requires the vocabulary modules again, as a descriptor module does.

local Postretro = require("postretro")
local Combat = require("./scripts/combat-lifecycle.luau")
local DamageTypes = require("./scripts/damage-types.luau")
local Defenses = require("./scripts/defenses.luau")

local read = Postretro.read
local progression = Combat.progression

local M = {}

M.player = Postretro.defineEntity({
  canonicalName = "player",
  components = {
    movement = {
      capsule = { radius = 0.2, halfHeight = 0.8, eyeHeight = 0.5 },
      ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 15.0, stepHeight = 0.5, maxSlope = 45.0 },
      air = { forwardSteer = 0.5, accel = 10, maxControlSpeed = 2, bunnyHop = true, jumps = 0, jumpVelocity = 6.86, jumpCeiling = 0.0 },
      fall = { terminalVelocity = 40.0 },
    },
    health = {
      max = 100,
      hitbox = { halfExtents = { 0.2, 1.0, 0.2 }, offset = { 0, 0, 0 } },
      layers = {
        {
          layer = Defenses.shield,
          capacity = read(progression.shieldCapacity):plus(50),
          rechargeDelayMs = 2500,
          rechargeRate = read(progression.shieldTech):times(4):plus(20),
          resist = { shock = read(progression.shockWard):times(-0.25):plus(1) },
        },
      },
    },
  },
})

M.referenceEnemyHealth = Postretro.defineEntity({
  canonicalName = "reference_enemy",
  components = {
    health = {
      max = 70,
      hitbox = { halfExtents = { 0.4, 0.9, 0.4 }, offset = { 0, 0.9, 0 } },
      zoneMultipliers = { head = 2.5, leg = 0.5 },
      layers = { { layer = Defenses.armor, capacity = 120 } },
      resist = { shock = 0.5 },
    },
  },
})

M.shieldedEnemy = Postretro.defineEntity({
  canonicalName = "reference_enemy_shielded",
  components = {
    health = {
      max = 70,
      hitbox = { halfExtents = { 0.4, 0.9, 0.4 }, offset = { 0, 0.9, 0 } },
      zoneMultipliers = { head = 2.5, leg = 0.5 },
      layers = { { layer = Defenses.shield, capacity = 60, rechargeRate = 15 } },
      resist = {},
    },
  },
})

local function gun(name: string, damageType: any)
  return Postretro.defineEntity({
    canonicalName = name,
    components = {
      weapon = {
        damage = 12,
        damageType = damageType,
        range = 80.0,
        primary = { trigger = "hold", recoveryMs = 110.0, steps = { Postretro.activation.shot() } },
        resolution = "hitscan",
      },
    },
  })
end

M.arcRifle = gun("arc_rifle", DamageTypes.shock)
M.acidRifle = gun("acid_rifle", DamageTypes.corrosive)

return M
