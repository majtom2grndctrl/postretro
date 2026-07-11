-- E18 authoring fixture — both trigger control surfaces in Luau.
--
-- `Primitive` reactions resolve targets from `tag`; handle methods build
-- per-id sequence steps. This mirrors trigger-fanout-fixture.ts exactly.

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
