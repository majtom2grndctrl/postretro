-- Luau twin of coop-activation-test.ts: the vault plates, the airlock gate and
-- the airlock HUD strip, built the same way.
-- See: context/lib/scripting.md §5 (Session players, Occupancy)

local Postretro = require("postretro")
local UI = require("postretro/ui")

local state = Postretro.getGameState()
local session = state.session
local occupancy = state.occupancy
local counts = session.playerCounts
local runtime = Postretro.runtime
-- Read at module scope, which the build-time bridge evaluates as install does.
local vaultPlates = occupancy:tag("vault-plates")

function setupLevel()
  local vaultDoor = Postretro.getMapEntities("mover", { tag = "vault-door" })
  local airlockDoor = Postretro.getMapEntities("mover", { tag = "airlock-door" })
  local function starts(doors)
    local steps = {}
    for _, door in doors do
      for _, step in door:start() do
        table.insert(steps, step)
      end
    end
    return steps
  end
  local solve = Postretro.defineReaction("vault.solve", { sequence = starts(vaultDoor) })
  local allIn = Postretro.defineReaction("airlock.allIn", { sequence = starts(airlockDoor) })

  -- The room's tag comes from the map, not a literal.
  local roomTag = "airlock"
  for _, volume in Postretro.getMapEntities("trigger") do
    for _, tag in volume.tags do
      if string.sub(tag, 1, 3) == "air" then
        roomTag = tag
      end
    end
  end
  local room = runtime.read(occupancy:tag(roomTag).occupants)
  local inPlay = runtime.read(counts.inPlay)

  return {
    reactions = { solve, allIn },
    crossings = {
      UI.onStateCrossing(vaultPlates.occupiedCount, { above = 1 }, { solve }),
      UI.onStateCrossing(
        runtime["and"](
          runtime.eq(runtime.read(counts.loading), 0),
          runtime["and"](runtime.gt(inPlay, 0), runtime.ge(room, inPlay))
        ),
        { allIn }
      ),
    },
    uiTrees = {
      UI.defineUiTree({
        name = "coop.airlock",
        alwaysOn = true,
        tree = UI.Tree(
          { anchor = "top", offset = { 0, 24 } },
          UI.Text({
            content = "",
            fontSize = 18,
            bind = UI.bindState(occupancy:tag(roomTag).occupants, { format = "IN THE AIRLOCK {}" }),
          })
        ),
      }),
    },
  }
end
