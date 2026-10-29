-- Ghost-link acceptance fixture (maps/ghost-links-luau.map). TypeScript twin: ghost-links.ts.
-- A two-player co-op set-piece: both players start in copy A. The first press
-- of copy A's purple pad sends whoever stands on it alone into copy B; every
-- later press flips both players between the copies until the return reunites
-- them. Every reaction is explicitly named, and member traversal preserves map
-- order.
local Postretro = require("postretro")
local Audio = require("postretro/audio")
local UI = require("postretro/ui")

-- Nested tables of steps flatten at any depth; mapping over members still needs
-- a loop.
local function map(members, build)
  local out = {}
  for _, member in ipairs(members) do table.insert(out, build(member)) end
  return out
end
local function bind(out, members, event, reactions)
  for _, member in ipairs(members) do table.insert(out, member:on(event, reactions)) end
end

function setupLevel(_ctx)
  local echoes = Postretro.getMapEntities("ghostLink", { tag = "vault_echo" }) -- map order
  -- One body: link steps, then the bus swell, a system step.
  local reveal = Postretro.defineReaction("echo.reveal", {
    map(echoes, function(l) return l:arm() end), -- also: l:disarm()
    -- Each named field ramps on its own; omitted fields keep their value and ramp. periodMs defaults to 0.
    map(echoes, function(l)
      return l:setColorFilter({ tint = { 1.0, 0.4, 0.3 }, opacity = 0.6, periodMs = 1500 })
    end),
    Audio.setGhostAudio({ volume = 0.9, lowPassHz = 5000, periodMs = 1500 }),
  })
  -- Narrow what a link shows for one scenario; omitted categories keep their value.
  local fight = Postretro.defineReaction("echo.fight",
    map(echoes, function(l) return l:setMembers({ projectiles = false }) end))

  -- Barrier puzzle: the button in back room B drops the barrier in copy A.
  -- Link echo_a lists the barrier's tag, so the player in copy B watches it drop.
  -- The same press raises the return button (below).
  local barrier = Postretro.getMapEntities("mover", { tag = "vault_barrier" })
  local open = Postretro.defineReaction("barrier.open", map(barrier, function(door) return door:start() end))

  -- North closet, copy A: its door sinks and its two knights wake. The monsters'
  -- sealed aggro gate (enabled_on_spawn 0) ignores everything else, damage
  -- included. Fifteen seconds after level load, or the first player to walk up
  -- to the barrier, whichever comes first. A start on a mover already moving
  -- or open does nothing and waking a woken monster changes nothing, so the
  -- second path opens nothing again.
  local northCloset = Postretro.defineReaction("closet.a1", {
    map(Postretro.getMapEntities("mover", { tag = "echo_closet_a1" }), function(door) return door:start() end),
    Postretro.npcs({ tag = "echo_monster_a1" }):update({ aggro = true }),
  })
  -- The timer counts from level load, so a host still waiting for a partner
  -- may meet the knights alone. The host opens the closet (`fire` reuses the
  -- reaction the barrier approach fires too); each connected machine sounds
  -- the klaxon once, on the host's timeline, and a player who joins later
  -- never hears it.
  local northTimer = Postretro.defineReaction("levelLoad", {
    Postretro.wait(15000),
    Postretro.fire(northCloset),
    UI.playSound("sfx/test_tone"),
  })

  -- Split: the first pad press arms both split teleporters, one over each pad,
  -- so only whoever stands on a pad crosses, and turns the split pads off. Both
  -- players start in copy A, so the crossing is always copy A's, tagged
  -- split_to_b. The pad press stands in for a game condition (a timer, a health
  -- threshold) and splits once: press it alone from the pad's centre.
  local splitPads = Postretro.getMapEntities("trigger", { tag = "split_pad" })
  local swapPads = Postretro.getMapEntities("trigger", { tag = "swap_pad" })
  local split = Postretro.getMapEntities("teleporter", { tag = "echo_split" })
  local splitToB = Postretro.getMapEntities("teleporter", { tag = "split_to_b" })
  local splitArm = Postretro.defineReaction("split.arm", {
    map(split, function(t) return t:arm() end),
    map(splitPads, function(p) return p:disarm() end),
  })
  -- The crossing turns the split off and only then turns the swap pads on. A
  -- swap pad armed on the press itself could fire on that same press.
  local splitSettle = Postretro.defineReaction("split.settle", {
    map(split, function(t) return t:disarm() end),
    map(swapPads, function(p) return p:arm() end),
  })

  -- The crossing into copy B starts both fights in one body. Copy A's south
  -- closet opens at once, so its two knights wait for whoever is left in or
  -- flips back to copy A; copy B's closets open three seconds later, so the
  -- arrival gets a beat to look around before the limitators wake.
  local splitLanded = Postretro.defineReaction("split.landed", {
    map(Postretro.getMapEntities("mover", { tag = "echo_closet_a2" }), function(door) return door:start() end),
    Postretro.npcs({ tag = "echo_monster_a2" }):update({ aggro = true }),
    Postretro.wait(3000),
    map(Postretro.getMapEntities("mover", { tag = "echo_closet_b" }), function(door) return door:start() end),
    Postretro.npcs({ tag = "echo_monster_b" }):update({ aggro = true }),
  })

  -- Flip: every swap pad press arms both seamless swap teleporters, so each
  -- player in a room lands in the other copy; the links stop ghosting the
  -- partner's projectiles. The first crossing disarms both directions, and
  -- the next press arms them again. The return turns the swap pads off.
  local swap = Postretro.getMapEntities("teleporter", { tag = "echo_swap" })
  local flip = Postretro.defineReaction("swap.arm", map(swap, function(t) return t:arm() end))
  local flipSettle = Postretro.defineReaction("swap.settle", map(swap, function(t) return t:disarm() end))

  -- Return: the barrier button raises a pedestal out of back room B's floor at
  -- once and arms its pad, whether or not copy B is clear. A press arms the
  -- classic teleporter over the pad, so only whoever stands there goes to room
  -- A. The arrival reunites the players: it disarms the return and turns the
  -- flip off for good, then after a three-second beat the closet in back room
  -- A opens and the final wave wakes. The barrier is already down: the press
  -- that raised the return button dropped it.
  local returnPedestal = Postretro.getMapEntities("mover", { tag = "return_pedestal" })
  local returnPads = Postretro.getMapEntities("trigger", { tag = "return_pad" })
  local returnGate = Postretro.getMapEntities("teleporter", { tag = "echo_return" })
  local rise = Postretro.defineReaction("return.rise", {
    map(returnPedestal, function(m) return m:start() end),
    map(returnPads, function(p) return p:arm() end),
  })
  local returnArm = Postretro.defineReaction("return.arm", map(returnGate, function(t) return t:arm() end))
  local returnSettle = Postretro.defineReaction("return.settle", {
    map(returnGate, function(t) return t:disarm() end),
    map(swapPads, function(p) return p:disarm() end),
    map(swap, function(t) return t:disarm() end),
    Postretro.wait(3000),
    map(Postretro.getMapEntities("mover", { tag = "final_closet_door" }), function(door) return door:start() end),
    Postretro.npcs({ tag = "final_wave" }):update({ aggro = true }),
  })

  local triggerEvents = {}
  bind(triggerEvents, Postretro.getMapEntities("trigger", { tag = "echo_plate" }), "enter", { reveal })
  bind(triggerEvents, Postretro.getMapEntities("trigger", { tag = "vault_button" }), "enter", { open, rise })
  bind(triggerEvents, Postretro.getMapEntities("trigger", { tag = "barrier_approach" }), "enter", { northCloset })
  bind(triggerEvents, splitPads, "enter", { splitArm })
  bind(triggerEvents, swapPads, "enter", { flip, fight })
  bind(triggerEvents, returnPads, "enter", { returnArm })
  local teleportEvents = {}
  bind(teleportEvents, split, "teleport", { splitSettle })
  bind(teleportEvents, splitToB, "teleport", { splitLanded })
  bind(teleportEvents, swap, "teleport", { flipSettle })
  bind(teleportEvents, returnGate, "teleport", { returnSettle })
  return {
    reactions = {
      reveal, fight, open, northCloset, northTimer, splitArm, splitSettle,
      splitLanded, flip, flipSettle, rise, returnArm, returnSettle,
    },
    triggerEvents = triggerEvents,
    teleportEvents = teleportEvents,
  }
end
