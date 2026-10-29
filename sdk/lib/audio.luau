--!strict
-- Audio subsystem vocabulary for `require("postretro/audio")`: the Ghost bus
-- setter.
-- See: context/lib/audio.md §1 (Ghost bus) · context/lib/scripting.md §10.4

local AudioSdk = {}

--- Change the mod-wide Ghost bus mix during play. Returns a system reaction:
--- a body on its own, or a step in a sequence or array body. It names no
--- target. Applies on the host at its next tick, in call order, and
--- replicates; on a connected client it does nothing. Each named field (`volume`, `highPassHz`,
--- `lowPassHz`, `drive`) ramps on its own over `periodMs` (default 0); an
--- omitted field keeps its value and any ramp in flight. A non-finite or
--- out-of-range field, a call naming no field, or targets that would leave
--- `highPassHz` at or above `lowPassHz` warn and drop the reaction.
function AudioSdk.setGhostAudio(fields: GhostAudioFields): SystemReactionStep
  -- A shallow copy, so a later edit to the author's table cannot reach the
  -- body; anything else passes through for the engine to refuse by name.
  local args: any = fields
  if type(fields) == "table" then
    args = {}
    for key, value in fields do
      args[key] = value
    end
  end
  return { primitive = "setGhostAudio", args = args }
end

return AudioSdk
