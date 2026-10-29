-- Luau twin of coop-presentation-test.ts. The player-event declarations below
-- mirror player-events.luau so this file is a standalone map entry point.
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

local function playerEventSetup(_ctx)
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


local alarm = Postretro.defineReaction("coop.plate.alarm", UI.playSound("sfx/test_tone"))
local flash = Postretro.defineReaction("coop.plate.flash", UI.flashScreen({ 0.2, 0.7, 1, 0.35 }, 250))
local shake = Postretro.defineReaction("coop.plate.shake", function(on)
  return UI.screenShake(6, 350, 18, { audience = on.trigger })
end)
local click = Postretro.defineReaction("coop.plate.click", function(on)
  return UI.playSound("fixtures/land", { audience = on.activators })
end)

local function append(out, values)
  for _, value in ipairs(values) do table.insert(out, value) end
end
local function raw(name, primitive, tag, args)
  return Postretro.defineReaction(name, { primitive = primitive, tag = tag, args = args })
end

function setupLevel(_ctx)
  local events = playerEventSetup(_ctx)
  local lights = Postretro.getMapEntities("light", { tag = "coop_prl_light" })
  local fogs = Postretro.getMapEntities("fog", { tag = "coop_fog" })
  local doors = Postretro.getMapEntities("mover", { tag = "coop_door" })
  local lightFade, fogFade, doorStart, deathPulse = {}, {}, {}, {}
  for _, light in ipairs(lights) do
    append(lightFade, light:fade({ from = 1, to = 0.2, periodMs = 5000 }))
    append(deathPulse, light:pulse({ min = 0.15, max = 1, periodMs = 1500 }))
  end
  for _, fog in ipairs(fogs) do
    append(fogFade, fog:fade({ from = 0.6, to = 0.1, periodMs = 5000 }))
  end
  for _, door in ipairs(doors) do append(doorStart, door:start()) end
  local atmosphere = {
    Postretro.defineReaction("coop.atmosphere.lights", { sequence = lightFade }),
    raw("coop.atmosphere.fogDensity", "setFogDensity", "coop_fog", { density = 0.6 }),
    Postretro.defineReaction("coop.atmosphere.fogFade", { sequence = fogFade }),
    raw("coop.atmosphere.fogParams", "setFogParams", "coop_fog", { tint = { 0.3, 0.6, 1 }, saturation = 0.7, minBrightness = 0.1, lightRange = 1.5 }),
    raw("coop.atmosphere.fogGlow", "setFogGlow", "coop_fog", { glow = 0.15 }),
    raw("coop.atmosphere.fogEdge", "setFogEdgeSoftness", "coop_fog", { edgeSoftness = 0.5 }),
    raw("coop.atmosphere.fogFalloff", "setFogFalloff", "coop_fog", { falloff = 2 }),
    raw("coop.atmosphere.mapEmitter", "setEmitterRate", "coop_map_emitter", { rate = 0 }),
    raw("coop.atmosphere.spin", "setSpinRate", "coop_map_emitter", { animation = { duration = 5, rate_curve = { 0.5, 6.28 } } }),
    raw("coop.atmosphere.descriptorLights", "setLightAnimation", "coop_descriptor_light", { periodMs = 2000, brightness = { 0.2, 1, 0.2 } }),
    raw("coop.atmosphere.descriptorEmitter", "setEmitterRate", "coop_descriptor_emitter", { rate = 0 }),
    raw("coop.atmosphere.prop", "setAnimationState", "coop_mesh_prop", { state = "alert" }),
    Postretro.defineReaction("coop.atmosphere.door", { sequence = doorStart }),
  }
  local mark = raw("coop.weapon.mark", "applyDamage", "coop_health_weapon", { amount = 12 })
  local reactions = {}
  append(reactions, events.reactions)
  append(reactions, { alarm, flash, shake, click })
  append(reactions, atmosphere)
  append(reactions, {
    Postretro.defineReaction("coop.enemy.dead", { sequence = deathPulse }),
    Postretro.defineReaction("coop.enemy.dead", UI.flashScreen({ 1, 0.2, 0.1, 0.3 }, 300, { audience = "everyone" })),
    Postretro.defineReaction("coop.enemy.progress", { progress = { tag = "coop_death_enemy", at = 1, fire = "coop.enemy.dead" } }),
    mark,
  })
  local triggerEvents = {}
  for _, plate in ipairs(Postretro.getMapEntities("trigger", { tag = "coop_plate" })) do
    table.insert(triggerEvents, plate:on("enter", { alarm, flash, shake, click }))
  end
  for _, trigger in ipairs(Postretro.getMapEntities("trigger", { tag = "coop_weapon_mark" })) do
    table.insert(triggerEvents, trigger:on("enter", { mark }))
  end
  for _, trigger in ipairs(Postretro.getMapEntities("trigger", { tag = "coop_atmosphere" })) do
    table.insert(triggerEvents, trigger:on("enter", atmosphere))
  end
  return { reactions = reactions, playerEvents = events.playerEvents, crossings = events.crossings, triggerEvents = triggerEvents }
end
