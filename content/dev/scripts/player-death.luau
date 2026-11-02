-- Author policy over the player life-state floor; twin of player-death.ts.
local Postretro = require("postretro")
local UI = require("postretro/ui")
local BLEED_OUT_MS = 15000
local player = Postretro.getGameState().player

local down = Postretro.defineReaction("life.down", function(on)
  return on.player:setLifeState("down")
end)
local respawn = Postretro.defineReaction("life.respawn", function(on)
  return on.player:respawn()
end)
local downedVignette = Postretro.defineReaction("life.downedVignette", UI.vignette(0.7, BLEED_OUT_MS, {0.65, 0, 0}))
local clearVignette = Postretro.defineReaction("life.clearVignette", UI.vignette(0, 1, {0.65, 0, 0}))

function setupLevel(_ctx)
  local isDown = Postretro.inLifeState("down")
  local expired = isDown["and"](isDown, Postretro.read(player.lifeStateMs):ge(BLEED_OUT_MS))
  local isUp = isDown["not"](isDown)
  -- Not-down guard: the edge goes false while down, so lethal damage in the respawn tick still fires it.
  local zeroHp = Postretro.read(player.health):le(0)
  local lethal = zeroHp["and"](zeroHp, isUp)
  return {
    reactions = {down, respawn, downedVignette, clearVignette},
    playerEvents = {
      Postretro.players():on(Postretro.becomes(lethal), {down}),
      Postretro.players():on(Postretro.becomes(expired), {respawn}),
    },
    crossings = {
      UI.onStateCrossing(isDown, {downedVignette}),
      UI.onStateCrossing(isUp, {clearVignette}),
    },
  }
end
