-- Luau twin of player-events.ts: the same per-player events, byte-identical
-- wire. Every reaction is named, because derived ids differ per runtime.
-- See: context/lib/scripting.md §12 (Player events)

local Postretro = require("postretro")
local UI = require("postretro/ui")

-- Mirror combat-lifecycle.ts and leveling.ts; the mod manifest registers both
-- stores, so these declarations only supply the slot refs.
local progression = Postretro.defineStore("progression", {
  xp = { type = "number", default = 0, perOwner = true, persist = true, network = "ownerPrivate" },
  teamKills = { type = "number", default = 0, network = "shared" },
})
local leveling = Postretro.defineStore("leveling", {
  level = { type = "number", default = 1, perOwner = true, network = "ownerPrivate" },
  lastLevelUpXp = { type = "number", default = 0, network = "shared" },
})

local player = Postretro.getGameState().player
local read = Postretro.read

-- One body per beat: the host's writes, then the presentation, which plays on
-- that player's machine.
local levelUp = Postretro.defineReaction("leveling.levelUp", function(on)
  return {
    on.player:addSlot(leveling.level, 1),
    UI.updateState(leveling.lastLevelUpXp, read(progression.xp:byPlayer(on.player))),
    UI.playSound("sfx/test_tone"),
    UI.flashScreen({ 1, 0.9, 0.3, 0.4 }, 300),
    UI.screenShake(6, 350, 18),
  }
end)
local scald = Postretro.defineReaction("heat.scald", function(on)
  return { on.player:damage(5), UI.playSound("fixtures/bite") }
end)
local cooled = Postretro.defineReaction("heat.cooled", UI.playSound("movers/door_close"))
local bleeding = Postretro.defineReaction("health.bleeding", UI.vignette(0.6, 800, { 0.8, 0, 0 }))

function setupLevel(_ctx)
  -- `and` is a Luau keyword, so boolean composition uses bracket spelling.
  local enoughXp = read(progression.xp):ge(100)
  local milestone = enoughXp["and"](enoughXp, read(leveling.level):lt(2))
  return {
    reactions = { levelUp, scald, cooled, bleeding },
    playerEvents = {
      -- Host decisions: run on the host, once per player. The guard
      -- (`level < 2`) goes false once the fire raises the level.
      Postretro.players():on(Postretro.becomes(milestone), { levelUp }),
      Postretro.players():on(Postretro.becomes(read(player.overheated)), { scald }), -- overheating burns the wielder
      Postretro.players():on(Postretro.ceases(read(player.overheated)), { cooled }),
    },
    crossings = {
      -- Own-state feedback: each machine, its own player.
      UI.onStateCrossing(player.health, { below = 25 }, { bleeding }),
    },
  }
end
