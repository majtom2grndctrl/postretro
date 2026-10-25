-- Ghost-link acceptance fixture (maps/ghost-links-luau.map). TypeScript twin: ghost-links.ts.
-- A two-player co-op set-piece. The first press of a purple pad sends whoever
-- stands on it alone into the other copy; every later press flips both players
-- between the copies until the return reunites them. Every reaction is
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

  -- North closet, copy A: its door sinks and its knights wake. The monsters'
  -- sealed aggro gate (enabled_on_spawn 0) ignores everything else, damage
  -- included. Fifteen seconds after level load, or the first player to walk up
  -- to the barrier, whichever comes first. A start on a mover already moving
  -- or open does nothing and waking a woken monster changes nothing, so the
  -- second path opens nothing again.
  local northDoor = Postretro.getMapEntities("mover", { tag = "echo_closet_a1" })
  local northSteps = each(northDoor, function(door) return door:start() end)
  table.insert(northSteps, Postretro.npcs({ tag = "echo_monster_a1" }):update({ aggro = true }))
  local northCloset = Postretro.defineReaction("closet.a1", { sequence = northSteps })
  -- The timer counts from level load, so a host still waiting for a partner
  -- may meet the knights alone. The fire step keeps the group wake out of the
  -- levelLoad body itself.
  local timerSteps = Postretro.wait(15000)
  append(timerSteps, Postretro.fire(northCloset))
  local northTimer = Postretro.defineReaction("levelLoad", { sequence = timerSteps })

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

  -- The crossing starts both fights. Copy A's south closet opens at once, so
  -- its knight waits for whoever is left in or flips back to copy A.
  local southDoor = Postretro.getMapEntities("mover", { tag = "echo_closet_a2" })
  local southSteps = each(southDoor, function(door) return door:start() end)
  table.insert(southSteps, Postretro.npcs({ tag = "echo_monster_a2" }):update({ aggro = true }))
  local southCloset = Postretro.defineReaction("closet.a2", { sequence = southSteps })
  -- Copy B's closets open three seconds after the crossing lands, so the
  -- arrival gets a beat to look around before the limitators wake.
  local closetsB = Postretro.getMapEntities("mover", { tag = "echo_closet_b" })
  local limitatorSteps = Postretro.wait(3000)
  append(limitatorSteps, each(closetsB, function(door) return door:start() end))
  table.insert(limitatorSteps, Postretro.npcs({ tag = "echo_monster_b" }):update({ aggro = true }))
  local limitators = Postretro.defineReaction("closet.b", { sequence = limitatorSteps })

  -- Flip: every swap pad press arms both seamless swap teleporters, so each
  -- player in a room lands in the other copy; the links stop ghosting the
  -- partner's projectiles. The first crossing disarms both directions, and
  -- the next press arms them again. The return turns the swap pads off.
  local swap = Postretro.getMapEntities("teleporter", { tag = "echo_swap" })
  local flip = Postretro.defineReaction("swap.arm", {
    sequence = each(swap, function(t) return t:arm() end),
  })
  local flipSettle = Postretro.defineReaction("swap.settle", {
    sequence = each(swap, function(t) return t:disarm() end),
  })

  -- Final wave: the barrier drops if it still stands, and after a three-second
  -- beat the closet in back room A opens and its monsters wake.
  local finalDoors = Postretro.getMapEntities("mover", { tag = "final_closet_door" })
  local waveSteps = each(barrier, function(door) return door:start() end)
  append(waveSteps, Postretro.wait(3000))
  append(waveSteps, each(finalDoors, function(door) return door:start() end))
  table.insert(waveSteps, Postretro.npcs({ tag = "final_wave" }):update({ aggro = true }))
  local wave = Postretro.defineReaction("wave.start", { sequence = waveSteps })

  -- Return: copy B cleared raises a pedestal in back room B and arms its pad. A
  -- press arms the classic teleporter over the pad, so only whoever stands
  -- there goes to room A. The arrival reunites the players: it disarms the
  -- return, turns the flip off for good and starts the wave.
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
  local returnSettleSteps = each(returnGate, function(t) return t:disarm() end)
  append(returnSettleSteps, each(swapPads, function(p) return p:disarm() end))
  append(returnSettleSteps, each(swap, function(t) return t:disarm() end))
  local returnSettle = Postretro.defineReaction("return.settle", { sequence = returnSettleSteps })

  local triggerEvents = {}
  bind(triggerEvents, Postretro.getMapEntities("trigger", { tag = "echo_plate" }), "enter", { reveal, swell })
  bind(triggerEvents, Postretro.getMapEntities("trigger", { tag = "vault_button" }), "enter", { open })
  bind(triggerEvents, Postretro.getMapEntities("trigger", { tag = "barrier_approach" }), "enter", { northCloset })
  bind(triggerEvents, splitPads, "enter", { splitArm })
  bind(triggerEvents, swapPads, "enter", { flip, fight })
  bind(triggerEvents, returnPads, "enter", { returnArm })
  local teleportEvents = {}
  bind(teleportEvents, split, "teleport", { splitSettle, southCloset, limitators })
  bind(teleportEvents, swap, "teleport", { flipSettle })
  bind(teleportEvents, returnGate, "teleport", { returnSettle, wave })
  return {
    reactions = {
      reveal, swell, fight, open, northCloset, northTimer, splitArm, splitSettle,
      southCloset, limitators, flip, flipSettle, wave, rise, clearB, returnArm, returnSettle,
    },
    triggerEvents = triggerEvents,
    teleportEvents = teleportEvents,
  }
end
