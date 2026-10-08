local Postretro = require("postretro")

local damagePresser = Postretro.defineReaction("fixture.presser.damage", function(on)
  return on.activators:damage(25)
end)
local disarmPlate = Postretro.defineReaction("fixture.presser.disarm", function(on)
  return { sequence = on.trigger:disarm() }
end)

function setupLevel(_ctx)
  local triggerEvents = {}
  for _, plate in Postretro.getMapEntities("trigger", { tag = "fixture_presser" }) do
    table.insert(triggerEvents, plate:on("enter", { damagePresser, disarmPlate }))
  end
  return {
    reactions = { damagePresser, disarmPlate },
    triggerEvents = triggerEvents,
  }
end
