-- Luau twin of area-lights-demo.ts: the same reaction, identical wire.
-- Both `skylight` panels are area lights (`lightType` "Area"); a pulse from
-- levelLoad reserves their animated bake. `sweep` on one fails with
-- InvalidArgument.
-- See: context/lib/scripting.md §12 (Entity addressing)

local Postretro = require("postretro")

function setupLevel(_ctx)
  local panels = Postretro.getMapEntities("light", { tag = "skylight" })
  local sequence = {}
  for _, p in panels do
    for _, step in p:pulse({ min = 0.3, max = 1.0, periodMs = 900 }) do
      table.insert(sequence, step)
    end
  end
  return {
    reactions = {
      Postretro.defineReaction("levelLoad", { sequence = sequence }),
    },
  }
end
