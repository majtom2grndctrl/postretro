-- Luau twin of trap-wave.ts: the same wave, byte-identical wire. Every
-- reaction is named, because derived ids differ per runtime.
-- See: context/lib/scripting.md §12 (Entity addressing)

local Postretro = require("postretro")

local STEP_MS = 250 -- gap between neighbouring traps
local PASSES = 3
local GAP_MS = 1500 -- pause between passes

local function append(steps: {any}, more: {any})
  for _, step in more do
    table.insert(steps, step)
  end
end

function setupLevel(_ctx)
  local traps = Postretro.getMapEntities("trap", { tag = "wave" })
  table.sort(traps, function(a, b)
    return a.position.x < b.position.x
  end)

  -- Sweep: PASSES passes, each firing the traps in order. fire() runs one execution.
  local pass = {}
  for i, t in traps do
    if i > 1 then
      append(pass, Postretro.wait(STEP_MS))
    end
    append(pass, t:fire())
  end
  local sweepSteps = {}
  for s = 1, PASSES do
    if s > 1 then
      append(sweepSteps, Postretro.wait(GAP_MS))
    end
    append(sweepSteps, pass)
  end
  local sweep = Postretro.defineReaction("wave.sweep", { sequence = sweepSteps })

  -- Looping wave: every trap repeats on one period; phase staggers them.
  local periodMs = #traps * STEP_MS + GAP_MS
  local onSteps = {}
  local offSteps = {}
  for i, t in traps do
    append(onSteps, t:start({ intervalMs = periodMs, phaseMs = (i - 1) * STEP_MS }))
    append(offSteps, t:stop())
  end
  local loopOn = Postretro.defineReaction("wave.loopOn", { sequence = onSteps })
  local loopOff = Postretro.defineReaction("wave.loopOff", { sequence = offSteps })

  local triggerEvents = {}
  for _, p in Postretro.getMapEntities("trigger", { tag = "sweep_plate" }) do
    table.insert(triggerEvents, p:on("enter", { sweep }))
  end
  for _, z in Postretro.getMapEntities("trigger", { tag = "loop_zone" }) do
    table.insert(triggerEvents, z:on("enter", { loopOn }))
    table.insert(triggerEvents, z:on("exit", { loopOff }))
  end

  return {
    reactions = { sweep, loopOn, loopOff },
    triggerEvents = triggerEvents,
  }
end
