--!nocheck
-- Reaction and sequence builders: `defineReaction`, the shared author-time
-- dispatch params and their opaque target tokens, `wait`/`fire` control steps,
-- and map-tag scoping.
-- Part chunk of `data_script.luau`: evaluated after `data_script/commands.luau`
-- (whose subject tokens it publishes as dispatch params) and before
-- `data_script.luau`, through the temporary `__postretroDataScriptParts`
-- bridge, and merged into its returned builder table. Type names refer to the
-- declarations in `data_script.luau`.
-- See: context/lib/scripting.md §7 (SDK library globals), §12

local ReactionsSdk = {}

-- The subject tokens carry their own verbs (`data_script/commands.luau`); they
-- never enter returned descriptors themselves.
local commands = __postretroDataScriptParts.commands
local ACTIVATORS_TARGET = commands.ACTIVATORS_TARGET
local PLAYER_TARGET = commands.PLAYER_TARGET
local TRIGGER_TARGET = commands.TRIGGER_TARGET
-- The emitter token carries its own wire spelling, so `playSound` (a separate
-- prelude chunk) lowers it without seeing this chunk's private tokens.
local EMITTER_TARGET = table.freeze({ __wire = "@emitter" })

-- One SDK-internal merged params table is shared by every author-time tracer.
-- Its value shape includes sibling opaque target tokens (activators, trigger,
-- player) alongside the input nodes.
local DISPATCH_PARAMS = table.freeze({
  rising = table.freeze({ op = "input", name = "@rising" }),
  dt = table.freeze({ op = "input", name = "@dt" }),
  activators = ACTIVATORS_TARGET,
  trigger = TRIGGER_TARGET,
  occupancy = table.freeze({ op = "input", name = "@occupancy" }),
  emitter = EMITTER_TARGET,
  player = PLAYER_TARGET,
})

--- Enroll the rest of this sequence body with the host scheduler and stop;
--- the remaining steps resume after `durationMs` (rounded up to whole
--- authoritative ticks). `interruptible` (default `false`) lets the
--- reaction's paired trigger Exit edge cancel the remaining steps while
--- parked.
function ReactionsSdk.wait(durationMs: number, opts: { interruptible: boolean? }?): {SequenceStep}
  local interruptible = if opts ~= nil and opts.interruptible ~= nil then opts.interruptible else false
  return { { id = "@wait", primitive = "wait", args = { durationMs = durationMs, interruptible = interruptible } } :: any }
end

--- Dispatch a named reaction by handle or name from inside a sequence body.
--- Resolves exactly as a trigger member's `on` resolves its `fire` entries. Luau has
--- no phantom-typed scope gate; a scoped reaction fired this way is rejected
--- at install (V4b), not at author time.
function ReactionsSdk.fire(reaction: Reaction<any> | string): {SequenceStep}
  local event = if type(reaction) == "string" then reaction else (reaction :: Reaction<any>).name
  return { { id = "@fire", primitive = "fire", args = { event = event } } :: any }
end

-- Order-stable serialization of a reaction body: table keys are emitted sorted
-- so two structurally identical bodies always serialize identically. Used only
-- to derive a deterministic auto-id; not a general JSON encoder.
local function stableStringify(value: any): string
  local t = type(value)
  if t == "table" then
    -- Distinguish array-like from map-like by checking a contiguous 1..n run.
    local n = #value
    local isArray = n > 0
    if isArray then
      for k in pairs(value) do
        if type(k) ~= "number" then
          isArray = false
          break
        end
      end
    end
    if isArray then
      local parts = {}
      for i = 1, n do
        parts[i] = stableStringify(value[i])
      end
      return "[" .. table.concat(parts, ",") .. "]"
    end
    local keys = {}
    for k in pairs(value) do
      table.insert(keys, tostring(k))
    end
    table.sort(keys)
    local parts = {}
    for _, k in ipairs(keys) do
      table.insert(parts, string.format("%q", k) .. ":" .. stableStringify((value :: any)[k]))
    end
    return "{" .. table.concat(parts, ",") .. "}"
  elseif t == "string" then
    return string.format("%q", value)
  else
    return tostring(value)
  end
end

-- Deterministic, run-stable id derived from a reaction body via FNV-1a (32-bit)
-- over its order-stable serialization. Re-running registration yields the same
-- id — crossings and the `onPress` wire form reference it. Bit ops use bit32.
-- NOTE: run-stable within Luau but NOT identical to the TS auto-id (different
-- stable-stringify). Do not assume cross-runtime id parity; use an explicit
-- name when the id must match between TS and Luau runtimes.
local function autoReactionId(descriptor: any): string
  local serialized = stableStringify(descriptor)
  local hash = 0x811c9dc5
  for i = 1, #serialized do
    hash = bit32.bxor(hash, string.byte(serialized, i))
    hash = bit32.band(hash * 0x01000193, 0xffffffff)
  end
  return string.format("reaction_%08x", hash)
end

--- Build a named reaction descriptor. Pure: returns a plain table, no FFI.
--- `descriptor` accepts exactly one body shape: `progress`, `primitive`, or
--- `sequence`. `name` is optional; when omitted a deterministic, run-stable id
--- is derived from the body. Use explicit names when TS and Luau scripts must
--- agree. The returned handle can be passed to button `onPress` or crossing
--- `fire` entries.
function ReactionsSdk.defineReaction(
  nameOrBody: string | (ProgressReactionDescriptor | PrimitiveReactionDescriptor | SequenceReactionDescriptor) | ((CrossingParams) -> (ProgressReactionDescriptor | PrimitiveReactionDescriptor | SequenceReactionDescriptor)),
  descriptor: (ProgressReactionDescriptor | PrimitiveReactionDescriptor | SequenceReactionDescriptor | ((CrossingParams) -> (ProgressReactionDescriptor | PrimitiveReactionDescriptor | SequenceReactionDescriptor)))?
): NamedReactionDescriptor
  local name: string
  local body: any
  if type(nameOrBody) == "string" then
    name = nameOrBody
    body = if type(descriptor) == "function" then (descriptor :: any)(DISPATCH_PARAMS) else descriptor
  else
    body = if type(nameOrBody) == "function" then (nameOrBody :: any)(DISPATCH_PARAMS) else nameOrBody
    name = autoReactionId(body)
  end
  local out: NamedReactionDescriptor = { name = name }
  -- Copy fields from the body into the flat shape the deserializer reads. Only
  -- the fields present on each variant are set; unused fields are left absent.
  local d = body :: any
  if d.progress ~= nil then
    out.progress = d.progress
  end
  if d.primitive ~= nil then
    out.primitive = d.primitive
  end
  if d.kind ~= nil then
    out.kind = d.kind
  end
  if d.tag ~= nil then
    out.tag = d.tag
  end
  if d.target ~= nil then
    out.target = d.target
  end
  if d.args ~= nil then
    out.args = d.args
  end
  if d.onComplete ~= nil then
    out.onComplete = d.onComplete
  end
  if d.sequence ~= nil then
    out.sequence = d.sequence
  end
  return out
end

--- Stamp a shared map-tag scope onto each reaction in a plain list. `tags` are
--- matched against `ModMapEntry.tags`; omit scoping for every level.
function ReactionsSdk.scopeReactions<S>(tags: {string}, list: {Reaction<S>}): {Reaction<S>}
  local out = {}
  for i, reaction in ipairs(list) do
    local scoped: NamedReactionDescriptor = { name = reaction.name, levels = tags }
    if reaction.progress ~= nil then
      scoped.progress = reaction.progress
    end
    if reaction.primitive ~= nil then
      scoped.primitive = reaction.primitive
    end
    if reaction.kind ~= nil then
      scoped.kind = reaction.kind
    end
    if reaction.tag ~= nil then
      scoped.tag = reaction.tag
    end
    if reaction.target ~= nil then
      scoped.target = reaction.target
    end
    if reaction.args ~= nil then
      scoped.args = reaction.args
    end
    if reaction.onComplete ~= nil then
      scoped.onComplete = reaction.onComplete
    end
    if reaction.sequence ~= nil then
      scoped.sequence = reaction.sequence
    end
    out[i] = scoped
  end
  return out
end

return {
  builders = ReactionsSdk,
  DISPATCH_PARAMS = DISPATCH_PARAMS,
}
