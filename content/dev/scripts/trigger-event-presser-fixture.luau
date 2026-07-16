local Postretro = require("postretro")

local damagePresser = Postretro.defineReaction("fixture.presser.damage", function(on)
  return Postretro.damage(on.activators, 25)
end)
local disarmPlate = Postretro.defineReaction("fixture.presser.disarm", function(on)
  return { sequence = Postretro.disarmTrigger(on.trigger) }
end)

function setupLevel(_ctx)
  return {
    reactions = { damagePresser, disarmPlate },
    triggerEvents = {
      Postretro.onTriggerEvent(
        { tag = "fixture_presser" },
        "enter",
        { damagePresser, disarmPlate }
      )
    }
  }
end
