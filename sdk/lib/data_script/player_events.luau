--!nocheck
-- Player-event builders: the `becomes` / `ceases` edge words and the
-- `players():on` descriptor. Pure data; effective only when returned under a
-- manifest's `playerEvents`.
-- Part chunk of `data_script.luau`: evaluated first, published through the
-- temporary `__postretroDataScriptParts` bridge (`commands.luau` reads
-- `playerEvent` from it for `players():on`), and its builders merged into the
-- returned table. Type names refer to the declarations in `data_script.luau`.
-- See: context/lib/scripting.md §12 (Player events)

local PlayerEventsSdk = {}

local boolNode = __postretroExpressionRefs.boolNode

-- Only edges built here lower; a hand-built `{ edge, condition }` is rejected
-- at author time rather than reaching the engine as a guess.
local edges = setmetatable({}, { __mode = "k" })

local function edgeWord(edge: string, cond: BoolRef): PlayerEventEdge
  local handle = table.freeze({})
  edges[handle] = { edge = edge, condition = boolNode(cond) }
  return handle
end

--- Fire when `cond` turns true for a player. A player first observed while
--- `cond` holds fires too.
function PlayerEventsSdk.becomes(cond: BoolRef): PlayerEventEdge
  return edgeWord("becomes", cond)
end

--- Fire when `cond` turns false for a player who was observed with it true.
function PlayerEventsSdk.ceases(cond: BoolRef): PlayerEventEdge
  return edgeWord("ceases", cond)
end

-- Build the `players():on` descriptor. Reaction handles lower to their
-- dispatch names; bare names pass through.
local function playerEvent(edge: PlayerEventEdge, fire: {any}, options: PlayerEventOptions?): PlayerEventDescriptor
  local data = if type(edge) == "table" then edges[edge] else nil
  if data == nil then
    error("players():on: `edge` must come from becomes(cond) or ceases(cond)", 2)
  end
  local names = {}
  for i, reaction in ipairs(fire) do
    names[i] = if type(reaction) == "string" then reaction else reaction.name
  end
  local out: PlayerEventDescriptor = { edge = data.edge, condition = data.condition, fire = names }
  if options ~= nil and options.levels ~= nil then
    out.levels = options.levels
  end
  return out
end

return {
  builders = PlayerEventsSdk,
  playerEvent = playerEvent,
}
