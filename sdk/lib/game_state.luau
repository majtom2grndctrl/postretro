--!strict
-- Engine-owned state reference surface. Captures the temporary bridge installed
-- by Rust into a pure closure, then removes the bridge before author code runs.

local GAME_STATE_BRIDGE_GLOBAL = "__postretroGameStateRefs"

local refs = _G[GAME_STATE_BRIDGE_GLOBAL]
if refs == nil then
  error("getGameState: missing engine state bridge")
end
_G[GAME_STATE_BRIDGE_GLOBAL] = nil

local GameStateSdk = {}

function GameStateSdk.getGameState(): any
  return refs
end

return GameStateSdk
