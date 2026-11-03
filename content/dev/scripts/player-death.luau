-- Author policy over the player life-state floor; twin of player-death.ts.
local Postretro = require("postretro")
local UI = require("postretro/ui")
local BLEED_OUT_MS = 15000
local player = Postretro.getGameState().player

-- Mirrors combat-lifecycle.ts; the mod manifest registers the store, so this
-- declaration only supplies the slot ref.
local progression = Postretro.defineStore("progression", {
  xp = { type = "number", default = 0, perOwner = true, persist = true, network = "ownerPrivate" },
  teamKills = { type = "number", default = 0, network = "shared" },
})

local down = Postretro.defineReaction("life.down", function(on)
  return on.player:setLifeState("down")
end)
local respawn = Postretro.defineReaction("life.respawn", function(on)
  return on.player:respawn()
end)
-- Borderlands revive: a teammate's completed assist stands the target up with
-- 30 health and pays the helper; a kill made while down is a second wind.
local revived = Postretro.defineReaction("life.revived", function(on)
  return { on.player:setLifeState("alive"), on.player:grantHealth(30) }
end)
local assistBonus = Postretro.defineReaction("life.assistBonus", function(on)
  return on.player:addSlot(progression.xp, 25)
end)
-- Its own impact event, not a row in `enemyDeath`, which the zombie override evicts.
local secondWind = Postretro.defineImpactEvent("second-wind", { tag = "enemy" }, function(impact)
  local target, source = impact.target, impact.source
  local healthBefore = target.healthBefore:gt(0)
  local killed = healthBefore["and"](healthBefore, target.healthAfter:le(0))
  return {
    {
      when = killed["and"](killed, source:inLifeState("down")),
      ["do"] = {
        source:setLifeState("alive"),
        source:grantHealth(40),
        Postretro.update(progression.xp:byPlayer(source), function(xp) return xp:plus(50) end),
      },
    },
  }
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
    reactions = {down, respawn, downedVignette, clearVignette, revived, assistBonus},
    events = {secondWind},
    playerEvents = {
      Postretro.players():on(Postretro.becomes(lethal), {down}),
      Postretro.players():on(Postretro.becomes(expired), {respawn}),
      Postretro.players():on(Postretro.assistReceived(), {revived}),
      Postretro.players():on(Postretro.assistGiven(), {assistBonus}),
    },
    crossings = {
      UI.onStateCrossing(isDown, {downedVignette}),
      UI.onStateCrossing(isUp, {clearVignette}),
    },
  }
end
