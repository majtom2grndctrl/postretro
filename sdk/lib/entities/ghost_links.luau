--!strict
-- Ghost-link member handles build host-only commands applied at the host's next tick.
-- See: context/lib/scripting.md §12 (Entity addressing) · context/lib/entity_model.md §7d
local GhostLinksSdk = {}
function GhostLinksSdk.wrapGhostLinkEntity(snapshot: GhostLinkEntity): GhostLinkHandle
  local id = snapshot.id
  -- Arguments pass through as authored: the engine validates them, alike for
  -- both runtimes, and drops a malformed step with a warning.
  return {
    id = id,
    position = snapshot.position,
    tags = snapshot.tags,
    arm = function(_self) return { { id = id, primitive = "armGhostLink", args = {} } } end,
    disarm = function(_self) return { { id = id, primitive = "disarmGhostLink", args = {} } } end,
    setMembers = function(_self, members)
      return { { id = id, primitive = "setGhostLinkMembers", args = members } }
    end,
    setColorFilter = function(_self, filter)
      return { { id = id, primitive = "setGhostLinkColorFilter", args = filter } }
    end,
  }
end
return GhostLinksSdk
