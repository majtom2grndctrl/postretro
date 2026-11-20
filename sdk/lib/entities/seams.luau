--!strict
-- Map level-seam handles build next-tick host-only commands and arrival event data.
-- See: context/lib/entity_model.md §7e (Level Seams) · context/lib/scripting.md §12
local SeamsSdk = {}
function SeamsSdk.wrapSeamEntity(snapshot: SeamEntity): SeamHandle
  local id = snapshot.id
  return {
    id = id,
    position = snapshot.position,
    tags = snapshot.tags,
    arm = function(_self) return { { id = id, primitive = "armSeam", args = {} } } end,
    disarm = function(_self) return { { id = id, primitive = "disarmSeam", args = {} } } end,
    on = function(_self, event, fire)
      local names = {}
      for i, reaction in ipairs(fire) do
        names[i] = if type(reaction) == "string" then reaction else reaction.name
      end
      return { seam = id, event = event, fire = names }
    end,
  }
end
return SeamsSdk
