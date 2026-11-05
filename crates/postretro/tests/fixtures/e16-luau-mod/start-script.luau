--!strict
-- The committed Luau mirror of E16's scripting surface: a mod that registers
-- the dev vocabulary, carries shielded and armored descriptors, and installs
-- the booster pads, the shield-break policy and a typed overload. Tests copy
-- this directory whole into a temporary mod root.

local Postretro = require("postretro")
local Combat = require("./scripts/combat-lifecycle.luau")
local DamageTypes = require("./scripts/damage-types.luau")
local Defenses = require("./scripts/defenses.luau")
local Entities = require("./scripts/entities.luau")

return Postretro.defineMod({
  name = "e16-luau-mod",
  id = "postretro.e16-luau-fixture",
  version = "1",
  defenseLayers = { Defenses.shield, Defenses.armor },
  damageTypes = {
    DamageTypes.ballistic,
    DamageTypes.plasma,
    DamageTypes.explosive,
    DamageTypes.shock,
    DamageTypes.corrosive,
    DamageTypes.flechette,
  },
  defaultDamageType = DamageTypes.ballistic,
  stores = { Combat.progression },
  entities = {
    Entities.player,
    Entities.referenceEnemyHealth,
    Entities.shieldedEnemy,
    Entities.arcRifle,
    Entities.acidRifle,
  },
  reactions = { Combat.booster, Combat.overload },
  triggerEvents = { Combat.boosterPads },
  events = { Combat.shieldBreak },
})
