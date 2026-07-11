-- Luau trigger-control authoring fixture.
-- See: context/lib/scripting.md §5

local Postretro = require("postretro")

function setupLevel(_ctx: any): LevelManifest
  local reactions = {
    Postretro.defineReaction("trigger.fixture.armByTag", {
      primitive = "armTrigger",
      tag = "fixture_tripwire",
      args = {},
    }),
    Postretro.defineReaction("trigger.fixture.disarmByTag", {
      primitive = "disarmTrigger",
      tag = "fixture_tripwire",
      args = {},
    }),
  }

  for _, trigger in Postretro.world:query({
    component = "trigger_volume",
    tag = "fixture_tripwire",
  }) do
    table.insert(reactions, Postretro.defineReaction("trigger.fixture.armById." .. trigger.id, {
      sequence = trigger:arm(),
    }))
    table.insert(reactions, Postretro.defineReaction("trigger.fixture.disarmById." .. trigger.id, {
      sequence = trigger:disarm(),
    }))
  end

  return { reactions = reactions }
end
