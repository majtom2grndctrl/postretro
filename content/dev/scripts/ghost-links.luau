-- Ghost-link acceptance fixture (maps/ghost-links-luau.map). TypeScript twin: ghost-links.ts.
-- The first press of a purple pad sends whoever stands on it alone into the
-- other copy; every later press swaps both players. Every reaction is
-- explicitly named, and member traversal preserves map order.
local Postretro = require("postretro")
local Audio = require("postretro/audio")

local function append(out, values)
  for _, value in ipairs(values) do table.insert(out, value) end
end
local function each(members, build)
  local out = {}
  for _, member in ipairs(members) do append(out, build(member)) end
  return out
end
local function bind(out, members, event, reactions)
  for _, member in ipairs(members) do table.insert(out, member:on(event, reactions)) end
end

function setupLevel(_ctx)
  local echoes = Postretro.getMapEntities("ghostLink", { tag = "vault_echo" }) -- map order
  local revealSteps = each(echoes, function(l) return l:arm() end) -- also: l:disarm()
  -- Each named field ramps on its own; omitted fields keep their value and ramp. periodMs defaults to 0.
  append(revealSteps, each(echoes, function(l)
    return l:setColorFilter({ tint = { 1.0, 0.4, 0.3 }, opacity = 0.6, periodMs = 1500 })
  end))
  local reveal = Postretro.defineReaction("echo.reveal", { sequence = revealSteps })
  -- Narrow what a link shows for one scenario; omitted categories keep their value.
  local fight = Postretro.defineReaction("echo.fight", {
    sequence = each(echoes, function(l) return l:setMembers({ projectiles = false }) end),
  })
  -- A mod-wide primitive with no target is a whole reaction body, like setSentiment.
  local swell = Postretro.defineReaction("echo.swell",
    Audio.setGhostAudio({ volume = 0.9, lowPassHz = 5000, periodMs = 1500 }))

  -- Barrier puzzle: the button in copy B drops the barrier in copy A. Link
  -- echo_a lists the barrier's tag, so the player in copy B watches it drop.
  local barrier = Postretro.getMapEntities("mover", { tag = "vault_barrier" })
  local open = Postretro.defineReaction("barrier.open", {
    sequence = each(barrier, function(door) return door:start() end),
  })

  -- Split: the first pad press arms both split teleporters, one over each pad,
  -- so only whoever stands on a pad crosses, and turns the split pads off.
  local splitPads = Postretro.getMapEntities("trigger", { tag = "split_pad" })
  local swapPads = Postretro.getMapEntities("trigger", { tag = "swap_pad" })
  local split = Postretro.getMapEntities("teleporter", { tag = "echo_split" })
  local splitArmSteps = each(split, function(t) return t:arm() end)
  append(splitArmSteps, each(splitPads, function(p) return p:disarm() end))
  local splitArm = Postretro.defineReaction("split.arm", { sequence = splitArmSteps })
  -- The crossing turns the split off and only then turns the swap pads on. A
  -- swap pad armed on the press itself could fire on that same press.
  local splitSettleSteps = each(split, function(t) return t:disarm() end)
  append(splitSettleSteps, each(swapPads, function(p) return p:arm() end))
  local splitSettle = Postretro.defineReaction("split.settle", { sequence = splitSettleSteps })

  -- Battle swap: the pad arms both seamless swap teleporters, so each player
  -- lands in the other copy, facing the monsters only the partner could see.
  -- The monsters wake, and the links stop ghosting the partner's projectiles.
  local swap = Postretro.getMapEntities("teleporter", { tag = "echo_swap" })
  local battleSteps = each(swap, function(t) return t:arm() end)
  table.insert(battleSteps, Postretro.npcs({ tag = "echo_monster" }):update({ aggro = true }))
  local battle = Postretro.defineReaction("battle.swap", { sequence = battleSteps })
  -- The first crossing disarms both directions, so walking out and back in
  -- does not swap again.
  local settle = Postretro.defineReaction("battle.settle", {
    sequence = each(swap, function(t) return t:disarm() end),
  })

  local triggerEvents = {}
  bind(triggerEvents, Postretro.getMapEntities("trigger", { tag = "echo_plate" }), "enter", { reveal, swell })
  bind(triggerEvents, Postretro.getMapEntities("trigger", { tag = "vault_button" }), "enter", { open })
  bind(triggerEvents, splitPads, "enter", { splitArm })
  bind(triggerEvents, swapPads, "enter", { battle, fight })
  local teleportEvents = {}
  bind(teleportEvents, split, "teleport", { splitSettle })
  bind(teleportEvents, swap, "teleport", { settle })
  return {
    reactions = { reveal, swell, fight, open, splitArm, splitSettle, battle, settle },
    triggerEvents = triggerEvents,
    teleportEvents = teleportEvents,
  }
end
