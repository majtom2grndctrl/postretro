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

  -- Barrier puzzle: the button in back room B drops the barrier in copy A.
  -- Link echo_a lists the barrier's tag, so the player in copy B watches it drop.
  local barrier = Postretro.getMapEntities("mover", { tag = "vault_barrier" })
  local open = Postretro.defineReaction("barrier.open", {
    sequence = each(barrier, function(door) return door:start() end),
  })

  -- Split: the first pad press arms both split teleporters, one over each pad,
  -- so only whoever stands on a pad crosses, and turns the split pads off. The
  -- pad press stands in for a game condition (a timer, a health threshold) and
  -- splits once: press it alone from the pad's centre.
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
  -- The closet doors in both copies sink and the monsters wake; the links stop
  -- ghosting the partner's projectiles. The monsters' sealed aggro gate
  -- (enabled_on_spawn 0) ignores everything else, damage included.
  local swap = Postretro.getMapEntities("teleporter", { tag = "echo_swap" })
  local closets = Postretro.getMapEntities("mover", { tag = "echo_closet_door" })
  local battleSteps = each(swap, function(t) return t:arm() end)
  append(battleSteps, each(closets, function(door) return door:start() end))
  table.insert(battleSteps, Postretro.npcs({ tag = "echo_monster" }):update({ aggro = true }))
  local battle = Postretro.defineReaction("battle.swap", { sequence = battleSteps })
  -- The first crossing disarms both directions, so walking out and back in
  -- does not swap again.
  local settle = Postretro.defineReaction("battle.settle", {
    sequence = each(swap, function(t) return t:disarm() end),
  })

  -- Final wave: the barrier drops, and after a three-second beat the closet in
  -- back room A opens and its monsters wake. It turns the ready button off. A
  -- start on a mover already moving or open does nothing, so whichever path
  -- fires it second starts nothing again.
  local readyPads = Postretro.getMapEntities("trigger", { tag = "ready_pad" })
  local finalDoors = Postretro.getMapEntities("mover", { tag = "final_closet_door" })
  local waveSteps = each(readyPads, function(p) return p:disarm() end)
  append(waveSteps, each(barrier, function(door) return door:start() end))
  append(waveSteps, Postretro.wait(3000))
  append(waveSteps, each(finalDoors, function(door) return door:start() end))
  table.insert(waveSteps, Postretro.npcs({ tag = "final_wave" }):update({ aggro = true }))
  local wave = Postretro.defineReaction("wave.start", { sequence = waveSteps })

  -- Ready button: copy A cleared raises a pedestal in the north closet the
  -- knights came from and arms its pad. A press starts the wave.
  local readyPedestal = Postretro.getMapEntities("mover", { tag = "ready_pedestal" })
  local readySteps = each(readyPedestal, function(m) return m:start() end)
  append(readySteps, each(readyPads, function(p) return p:arm() end))
  local ready = Postretro.defineReaction("ready.rise", { sequence = readySteps })
  local clearA = Postretro.defineReaction("clear.a", { progress = { tag = "echo_monster_a", at = 1.0, fire = "ready.rise" } })

  -- Return: copy B cleared raises a pedestal in back room B and arms its pad. A
  -- press arms the classic teleporter over the pad, so only whoever stands
  -- there goes to room A; the arrival disarms it and starts the wave.
  local returnPedestal = Postretro.getMapEntities("mover", { tag = "return_pedestal" })
  local returnPads = Postretro.getMapEntities("trigger", { tag = "return_pad" })
  local returnGate = Postretro.getMapEntities("teleporter", { tag = "echo_return" })
  local riseSteps = each(returnPedestal, function(m) return m:start() end)
  append(riseSteps, each(returnPads, function(p) return p:arm() end))
  local rise = Postretro.defineReaction("return.rise", { sequence = riseSteps })
  local clearB = Postretro.defineReaction("clear.b", { progress = { tag = "echo_monster_b", at = 1.0, fire = "return.rise" } })
  local returnArm = Postretro.defineReaction("return.arm", {
    sequence = each(returnGate, function(t) return t:arm() end),
  })
  local returnSettle = Postretro.defineReaction("return.settle", {
    sequence = each(returnGate, function(t) return t:disarm() end),
  })

  local triggerEvents = {}
  bind(triggerEvents, Postretro.getMapEntities("trigger", { tag = "echo_plate" }), "enter", { reveal, swell })
  bind(triggerEvents, Postretro.getMapEntities("trigger", { tag = "vault_button" }), "enter", { open })
  bind(triggerEvents, splitPads, "enter", { splitArm })
  bind(triggerEvents, swapPads, "enter", { battle, fight })
  bind(triggerEvents, readyPads, "enter", { wave })
  bind(triggerEvents, returnPads, "enter", { returnArm })
  local teleportEvents = {}
  bind(teleportEvents, split, "teleport", { splitSettle })
  bind(teleportEvents, swap, "teleport", { settle })
  bind(teleportEvents, returnGate, "teleport", { returnSettle, wave })
  return {
    reactions = {
      reveal, swell, fight, open, splitArm, splitSettle, battle, settle,
      wave, ready, clearA, rise, clearB, returnArm, returnSettle,
    },
    triggerEvents = triggerEvents,
    teleportEvents = teleportEvents,
  }
end
