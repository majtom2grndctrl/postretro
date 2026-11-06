--!strict
-- Luau twin of `damage-types.ts`: the dev mod's damage types, by matrix.

local Postretro = require("postretro")

return {
  ballistic = Postretro.defineDamageType("ballistic", { armor = 0.8 }),
  plasma = Postretro.defineDamageType("plasma", { shield = 1.5, armor = 0.5 }),
  explosive = Postretro.defineDamageType("explosive", { armor = 1.5, health = 1.25, shield = 0.75 }),
  shock = Postretro.defineDamageType("shock", { shield = 2, armor = 0.5, health = 0.75 }),
  corrosive = Postretro.defineDamageType("corrosive", { shield = 0.5, armor = 2 }),
  flechette = Postretro.defineDamageType("flechette", { shield = "bypass", armor = 0 }),
}
