-- Teleporter acceptance fixture. TypeScript twin: teleporters.ts.
-- Every reaction is explicitly named, and member traversal preserves map order.
local Postretro = require("postretro")

-- The dev mod registers this base. Only its closet-specific override installs.
local enemyDeath = Postretro.defineImpactEvent("enemy-death", { tag = "enemy" }, function(_impact)
  return {}
end)
local closetDeath = enemyDeath:override({ tag = "teleport_closet" }, function(impact)
  local killed = impact.target.healthBefore:gt(0)
  killed = killed["and"](killed, impact.target.healthAfter:le(0))
  return { { when = killed, do = { impact.target:despawn() } } }
end)

local function append(out, values)
  for _, value in ipairs(values) do table.insert(out, value) end
end
local function commands(members, action)
  local out = {}
  for _, member in ipairs(members) do append(out, member[action](member)) end
  return out
end
local function bind(out, members, event, reactions)
  for _, member in ipairs(members) do table.insert(out, member:on(event, reactions)) end
end

function setupLevel(_ctx)
  local gate = Postretro.getMapEntities("teleporter", { tag = "vault_gate" })
  local shift = Postretro.getMapEntities("teleporter", { tag = "era_shift" })
  local stress = Postretro.getMapEntities("teleporter", { tag = "stress_route" })
  if #stress > 0 then
    local arm = Postretro.defineReaction("stress.arm", { sequence = commands(stress, "arm") })
    local triggerEvents = {}
    bind(triggerEvents, Postretro.getMapEntities("trigger", { tag = "stress_button" }), "enter", { arm })
    return { reactions = { arm }, triggerEvents = triggerEvents }
  end
  -- Matched baselines keep this script and simply have no teleporter members.
  if #gate == 0 and #shift == 0 then
    return { reactions = {}, triggerEvents = {}, teleportEvents = {} }
  end

  local button = Postretro.getMapEntities("trigger", { tag = "vault_button" })
  local plate = Postretro.getMapEntities("trigger", { tag = "closet_plate" })
  local doors = Postretro.getMapEntities("mover", { tag = "closet_door" })
  local open = Postretro.defineReaction("gate.open", { sequence = commands(gate, "arm") })
  local shut = Postretro.defineReaction("gate.shut", { sequence = commands(gate, "disarm") })
  local cleared = Postretro.defineReaction("closet.cleared", { progress = { tag = "closet", at = 1.0, fire = "gate.open" } })
  local revealSteps = commands(doors, "start")
  table.insert(revealSteps, Postretro.npcs({ tag = "closet" }):update({ aggro = true }))
  local reveal = Postretro.defineReaction("closet.reveal", { sequence = revealSteps })
  local timeShift = Postretro.defineReaction("era.shift", { sequence = commands(shift, "arm") })
  local reset = Postretro.defineReaction("era.reset", { sequence = commands(shift, "disarm") })
  local oneWay = Postretro.defineReaction("gate.used", function(on)
    return on.teleporter:disarm()
  end)
  local triggerEvents = {}
  bind(triggerEvents, button, "enter", { open })
  bind(triggerEvents, plate, "enter", { shut, reveal })
  bind(triggerEvents, Postretro.getMapEntities("trigger", { tag = "era_button" }), "enter", { timeShift })
  bind(triggerEvents, Postretro.getMapEntities("trigger", { tag = "era_plate" }), "enter", { timeShift })
  bind(triggerEvents, Postretro.getMapEntities("trigger", { tag = "era_reset" }), "enter", { reset })
  local teleportEvents = {}
  bind(teleportEvents, gate, "teleport", { oneWay })
  return {
    reactions = { open, shut, cleared, reveal, timeShift, reset, oneWay },
    events = { closetDeath },
    triggerEvents = triggerEvents,
    teleportEvents = teleportEvents,
  }
end
