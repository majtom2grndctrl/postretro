--!strict
-- Luau twin of `attack-tokens.ts`: the dev mod's attack tokens, by default
-- budget. Luau names each declaration explicitly. Requiring this module again
-- only returns the handles; `defineMod` registers the declaration in force.

local Postretro = require("postretro")

return {
  melee = Postretro.defineAttackToken("melee", { limit = 3 }),
  ranged = Postretro.defineAttackToken("ranged", { limit = 2 }),
  heavy = Postretro.defineAttackToken("heavy", { limit = 1, refreshMs = 4000 }),
}
