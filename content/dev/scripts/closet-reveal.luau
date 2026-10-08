-- Luau twin of closet-reveal.ts: the same set piece, byte-identical wire.
-- Every reaction is named, because derived ids differ per runtime.
-- See: context/lib/scripting.md §12

local Postretro = require("postretro")
local UI = require("postretro/ui")

-- Mirrors closet-store.ts; the mod manifest registers the store, so this
-- declaration only supplies the slot refs.
local closetStore = Postretro.defineStore("closetReveal", {
  alarm = { type = "number", default = 0, network = "shared" },
})

local function append(steps: {any}, more: {any})
  for _, step in more do
    table.insert(steps, step)
  end
end

-- System-targeted state write: a primitive reaction, dispatched via `fire()`.
local raiseAlarm = Postretro.defineReaction("closet.raiseAlarm", UI.updateState(closetStore.alarm, 1))
-- Stepping off the plate heals whoever stepped off.
local patchUp = Postretro.defineReaction("closet.patchUp", function(on)
  return on.activators:grantHealth(25)
end)
-- Every seat-bound player, resolved when the reaction fires.
local resupply = Postretro.defineReaction("closet.resupply", Postretro.players():grantAmmo("shells.buck", 8))

function setupLevel(_ctx)
  local door = Postretro.getMapEntities("mover", { tag = "closet_door" }) -- members, fixed at install
  local closets = Postretro.getMapEntities("spawner", { tag = "closet_spawner" }) -- spawned_tags "closet"
  local plate = Postretro.getMapEntities("trigger", { tag = "closet_reveal_plate" })
  local alarmLights = Postretro.getMapEntities("light", { tag = "closet_alarm" })
  table.sort(alarmLights, function(a, b)
    return a.position.x < b.position.x -- inspect in Luau
  end)
  -- Group: the placed residents and whatever the spawners release.
  local closet = Postretro.npcs({ tag = "closet" })

  -- Client-local presentation: pulses the alarm lights, staggered west to
  -- east, whenever the replicated alarm slot reads nonzero.
  local pulses = {}
  for i, l in alarmLights do
    append(pulses, l:pulse({ min = 0.3, max = 1.0, periodMs = 400 + (i - 1) * 50 }))
  end
  local alarmLight = Postretro.defineReaction("closet.alarmLight", { sequence = pulses })

  -- One authored beat: raise the alarm now, hold, then open, release and
  -- rouse the closet. Stepping off the plate during the hold cancels the rest.
  local steps = {}
  append(steps, Postretro.fire(raiseAlarm))
  append(steps, Postretro.wait(800, { interruptible = true }))
  for _, m in door do
    append(steps, m:start())
  end
  for _, s in closets do
    append(steps, s:fire())
  end
  table.insert(steps, closet:update({ aggro = true })) -- whoever exists when this step runs
  table.insert(steps, closet:damage(5))
  append(steps, Postretro.fire(resupply))
  local reveal = Postretro.defineReaction("closet.timedReveal", { sequence = steps })

  local triggerEvents = {}
  for _, t in plate do
    table.insert(triggerEvents, t:on("enter", { reveal }))
    table.insert(triggerEvents, t:on("exit", { patchUp }))
  end

  return {
    reactions = { reveal, raiseAlarm, alarmLight, patchUp, resupply },
    triggerEvents = triggerEvents,
    crossings = { UI.onStateCrossing(closetStore.alarm, { above = 0 }, { alarmLight }) },
  }
end
