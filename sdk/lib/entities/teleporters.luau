--!strict
-- Map teleporter handles build next-tick authoritative commands and event data.
-- See: context/lib/scripting.md §12 (Teleport events)
local TeleportersSdk = {}
function TeleportersSdk.wrapTeleporterEntity(snapshot: TeleporterEntity): TeleporterHandle
  local id = snapshot.id
  return {
    id = id,
    position = snapshot.position,
    tags = snapshot.tags,
    arm = function(_self) return { { id = id, primitive = "armTeleporter", args = {} } } end,
    disarm = function(_self) return { { id = id, primitive = "disarmTeleporter", args = {} } } end,
    on = function(_self, event, fire)
      local names = {}
      for i, reaction in ipairs(fire) do
        names[i] = if type(reaction) == "string" then reaction else reaction.name
      end
      return { teleporter = id, event = event, fire = names }
    end,
  }
end
return TeleportersSdk
