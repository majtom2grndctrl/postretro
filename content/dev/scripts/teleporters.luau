-- Teleporter acceptance fixture. TypeScript twin: teleporters.ts.
-- Every reaction is explicitly named, and member traversal preserves map order.
local Postretro = require("postretro")

-- The purple gate's lamp brightness: dim while the gate is disarmed, bright while armed.
local LAMP_DISARMED = 0.2
local LAMP_ARMED = 1.0

-- The dev mod registers this base. Only its closet-specific override installs.
local enemyDeath = Postretro.defineImpactEvent("enemy-death", { tag = "enemy" }, function(_impact)
  return {}
end)
local closetDeath = enemyDeath:override({ tag = "teleport_closet" }, function(impact)
  local killed = impact.target.healthBefore:gt(0)
  killed = killed["and"](killed, impact.target.healthAfter:le(0))
  return { { when = killed, ["do"] = { impact.target:despawn() } } }
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
  -- Every gate arm and disarm below also sets the lamp, so it tracks the armed state.
  local lamps = Postretro.getMapEntities("light", { tag = "gate_lamp" })
  local function lamp(armed)
    local steps = {}
    for _, light in ipairs(lamps) do
      table.insert(steps, {
        id = light.id,
        primitive = "setLightAnimation",
        args = { periodMs = 1000, brightness = { if armed then LAMP_ARMED else LAMP_DISARMED } },
      })
    end
    return steps
  end
  -- The gate starts disarmed (`enabled_on_spawn` 0), so its lamp starts dim.
  local lampAtLoad = Postretro.defineReaction("levelLoad", { sequence = lamp(false) })
  -- Matched baselines keep this script and simply have no teleporter members.
  -- The hub baseline keeps the lamp, which stays dim with no gate to arm.
  if #gate == 0 and #shift == 0 then
    return { reactions = if #lamps > 0 then { lampAtLoad } else {}, triggerEvents = {}, teleportEvents = {} }
  end

  local button = Postretro.getMapEntities("trigger", { tag = "vault_button" })
  local plate = Postretro.getMapEntities("trigger", { tag = "closet_plate" })
  local doors = Postretro.getMapEntities("mover", { tag = "closet_door" })
  local function gateState(armed)
    local steps = commands(gate, if armed then "arm" else "disarm")
    append(steps, lamp(armed))
    return steps
  end
  local open = Postretro.defineReaction("gate.open", { sequence = gateState(true) })
  local shut = Postretro.defineReaction("gate.shut", { sequence = gateState(false) })
  local cleared = Postretro.defineReaction("closet.cleared", { progress = { tag = "closet", at = 1.0, fire = "gate.open" } })
  local revealSteps = commands(doors, "start")
  table.insert(revealSteps, Postretro.npcs({ tag = "closet" }):update({ aggro = true }))
  local reveal = Postretro.defineReaction("closet.reveal", { sequence = revealSteps })
  local timeShift = Postretro.defineReaction("era.shift", { sequence = commands(shift, "arm") })
  local reset = Postretro.defineReaction("era.reset", { sequence = commands(shift, "disarm") })
  local oneWay = Postretro.defineReaction("gate.used", function(on)
    local steps = { on.teleporter:disarm() }
    append(steps, lamp(false))
    return { sequence = steps }
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
    reactions = { lampAtLoad, open, shut, cleared, reveal, timeShift, reset, oneWay },
    events = { closetDeath },
    triggerEvents = triggerEvents,
    teleportEvents = teleportEvents,
  }
end
