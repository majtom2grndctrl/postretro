--!strict
-- Luau mirror of the dev mod's E16 combat rules: the shield progression
-- slots, the booster pads and `shieldBreak`. Requires the vocabulary modules
-- again; each require only returns handles, and `defineMod` registers the
-- declarations in force.

local Postretro = require("postretro")
local DamageTypes = require("./scripts/damage-types.luau")
local Defenses = require("./scripts/defenses.luau")

local M = {}

M.progression = Postretro.defineStore("progression", {
  xp = { type = "number", default = 0, perOwner = true, persist = true, network = "ownerPrivate" },
  teamKills = { type = "number", default = 0, network = "shared" },
  shieldCapacity = { type = "number", default = 0, perOwner = true, persist = true, network = "ownerPrivate" },
  shieldTech = { type = "number", default = 0, perOwner = true, persist = true, network = "ownerPrivate" },
  shockWard = { type = "number", default = 0, perOwner = true, persist = true, network = "ownerPrivate" },
})

-- A booster pad adds 25 shield.
M.booster = Postretro.defineReaction("combat.shieldBooster", function(on)
  return on.activators:grantLayer(Defenses.shield, 25)
end)
M.boosterPads = Postretro.defineTriggerEvent({ tag = "shield_booster", event = "enter", fire = { M.booster } })

-- Refund the attacker a quarter of a shock hit that breaks a layer and
-- leaves a live target's shield empty; mark a target whose armor is gone.
M.shieldBreak = Postretro.defineImpactEvent("shield-break", { tag = "enemy" }, function(impact)
  local shield = impact.target:defense(Defenses.shield)
  local broke = impact.broke["and"](impact.broke, impact.type(DamageTypes.shock))
  broke = broke["and"](broke, shield.current:le(0))
  broke = broke["and"](broke, impact.target.healthBefore:gt(0))
  return {
    Postretro.when(broke, { impact.source:grantLayer(Defenses.shield, impact.absorbed:times(0.25)) }),
    Postretro.when(
      impact.target:defense(Defenses.armor).current:le(0),
      { impact.target:setState("exposed", 1) }
    ),
  }
end)

-- Typed script damage: a 40-point shock overload on every turret.
M.overload = Postretro.defineReaction("turret.overload", Postretro.npcs({ tag = "turret" }):damage(40, { type = DamageTypes.shock }))

return M
