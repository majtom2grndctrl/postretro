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

-- Stepping off the plate heals whoever stepped off.
local patchUp = Postretro.defineReaction("closet.patchUp", function(on)
  return on.activators:grantHealth(25)
end)

-- Nested tables of steps flatten at any depth; mapping over members still
-- needs a loop.
local function map(members, build)
  local out = {}
  for i, member in ipairs(members) do table.insert(out, build(member, i)) end
  return out
end

function setupLevel(_ctx)
  local door = Postretro.getMapEntities("mover", { tag = "closet_door" }) -- members, fixed at install
  local closets = Postretro.getMapEntities("spawner", { tag = "closet_spawner" }) -- spawned_tags "closet enemy"
  local plate = Postretro.getMapEntities("trigger", { tag = "closet_reveal_plate" })
  local alarmLights = Postretro.getMapEntities("light", { tag = "closet_alarm" })
  table.sort(alarmLights, function(a, b)
    return a.position.x < b.position.x -- inspect in Luau
  end)
  -- Group: the placed residents and whatever the spawners release.
  local closet = Postretro.npcs({ tag = "closet" })

  -- Client-local presentation: pulses the alarm lights, staggered west to
  -- east, whenever the replicated alarm slot reads nonzero.
  local alarmLight = Postretro.defineReaction("closet.alarmLight", map(alarmLights, function(l, i)
    return l:pulse({ min = 0.3, max = 1.0, periodMs = 400 + (i - 1) * 50 })
  end))

  -- One authored beat: raise the alarm now, hold, then open, release and
  -- rouse the closet, and resupply every seat-bound player. Stepping off the
  -- plate during the hold cancels the rest.
  local reveal = Postretro.defineReaction("closet.timedReveal", {
    UI.updateState(closetStore.alarm, 1), -- a system step: the replicated alarm write
    Postretro.wait(800, { interruptible = true }),
    map(door, function(m) return m:start() end),
    map(closets, function(s) return s:fire() end),
    closet:update({ aggro = true }), -- whoever exists when this step runs
    closet:damage(5),
    Postretro.players():grantAmmo("shells.buck", 8), -- resolved when this step runs
  })

  local triggerEvents = {}
  for _, t in plate do
    table.insert(triggerEvents, t:on("enter", { reveal }))
    table.insert(triggerEvents, t:on("exit", { patchUp }))
  end

  return {
    reactions = { reveal, alarmLight, patchUp },
    triggerEvents = triggerEvents,
    crossings = { UI.onStateCrossing(closetStore.alarm, { above = 0 }, { alarmLight }) },
  }
end
