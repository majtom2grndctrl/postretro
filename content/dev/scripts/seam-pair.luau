-- Level seam pair script (map seam-pair-a): the worked level_seam scripting
-- example from docs/level-seams.md. TypeScript twin: seam-pair.ts.
-- The seam a player arrives by disarms; stepping on the airlock button arms it again.
local Postretro = require("postretro")

function setupLevel(_ctx)
  local airlock = Postretro.getMapEntities("seam", { tag = "airlock" })
  local steps = {}
  for _, s in ipairs(airlock) do
    for _, step in ipairs(s:arm()) do
      table.insert(steps, step)
    end
  end
  local cycle = Postretro.defineReaction("airlock.cycle", { sequence = steps })
  local sealed = Postretro.defineReaction("airlock.sealed", function(on)
    return on.seam:disarm()
  end)
  local triggerEvents = {}
  for i, t in ipairs(Postretro.getMapEntities("trigger", { tag = "airlock_button" })) do
    triggerEvents[i] = t:on("enter", { cycle })
  end
  local seamEvents = {}
  for i, s in ipairs(airlock) do
    seamEvents[i] = s:on("arrive", { sealed })
  end
  return {
    reactions = { cycle, sealed },
    triggerEvents = triggerEvents,
    seamEvents = seamEvents,
  }
end
