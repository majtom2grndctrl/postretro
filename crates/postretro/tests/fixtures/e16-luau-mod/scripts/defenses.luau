--!strict
-- Luau twin of `defenses.ts`: the dev mod's defense layer kinds. Luau names
-- each declaration explicitly. Requiring this module again only returns the
-- handles; `defineMod` registers the declaration in force.

local Postretro = require("postretro")

return {
  shield = Postretro.defineDefenseLayer("shield"),
  armor = Postretro.defineDefenseLayer("armor"),
}
