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
local TELEPORTER_TARGET = commands.TELEPORTER_TARGET
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
  teleporter = TELEPORTER_TARGET,
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
    -- Raw reads only, so a passed-through command target's strict metatable
    -- never fires.
    local n = rawlen(value)
    local isArray = n > 0
    if isArray then
      for k in next, value do
        if type(k) ~= "number" then
          isArray = false
          break
        end
      end
    end
    if isArray then
      local parts = {}
      for i = 1, n do
        parts[i] = stableStringify(rawget(value, i))
      end
      return "[" .. table.concat(parts, ",") .. "]"
    end
    local keys = {}
    for k in next, value do
      table.insert(keys, tostring(k))
    end
    table.sort(keys)
    local parts = {}
    for _, k in ipairs(keys) do
      table.insert(parts, string.format("%q", k) .. ":" .. stableStringify(rawget(value, k)))
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

-- Every `defineReaction` message here is the same literal the TS SDK throws;
-- only an interpolated runtime type name, reaction name or primitive varies. A
-- body that is none of `progress`, `primitive`, `sequence` or a list passes
-- through, so the manifest parser skips that one reaction with a warning. What
-- a list holds, and a `sequence` that is not a list, raises here. Each raise
-- carries `level`, the `error` level of the author's `defineReaction` call
-- from the raising function, so the position names the author's line.
local SEQUENCE_SHAPE_ERROR = "defineReaction: `sequence` must be a list of steps"
local MISSING_ENTRY_ERROR = "defineReaction: a list of steps has a missing entry"
local RUNTIME_VALUE_ERROR = "defineReaction: a runtime value is not a step"

-- How `t` reads as a list of steps: "list" when empty or keyed exactly 1..n,
-- "holey" when keyed by positive integers with a gap (a `nil` entry in a table
-- constructor), else nil. Probes raw keys only, so no `__index` fires. A table
-- whose metatable serves fields (a command target, a state ref) is never a
-- list, though an empty one has no raw keys; mlua's array marker hides its
-- metatable and passes. A trailing `nil` leaves no key, so it reads as absent.
local function listShape(t: any): string?
  local mt = getmetatable(t)
  if type(mt) == "table" and rawget(mt, "__index") ~= nil then
    return nil
  end
  local count = 0
  local highest = 0
  for key in next, t do
    if type(key) ~= "number" or key < 1 or key % 1 ~= 0 then
      return nil
    end
    count += 1
    if key > highest then
      highest = key
    end
  end
  return if count == highest then "list" else "holey"
end

-- The body key a table carries beside a list part, if any: the author meant
-- one or the other.
local function mixedKey(t: any): string?
  if rawget(t, 1) == nil then
    return nil
  end
  for _, key in ipairs({ "primitive", "progress", "sequence" }) do
    if rawget(t, key) ~= nil then
      return key
    end
  end
  return nil
end

local function mixedError(key: string): string
  return string.format("defineReaction: a list of steps cannot also carry `%s`", key)
end

-- Append `list`'s steps to `out` in authored order, flattening nested lists at
-- any depth. A table holding `primitive` is a step and a list is recursed
-- into; anything else raises, naming `defineReaction`. Every probe is raw, so
-- a handle with a strict metatable raises this message, not its own. `list`
-- is a "list" by `listShape`.
local function flattenSteps(list: any, out: {any}, level: number): {any}
  for i = 1, rawlen(list) do
    local entry = rawget(list, i)
    if entry == nil then
      error(MISSING_ENTRY_ERROR, level)
    end
    if type(entry) ~= "table" then
      error(string.format("defineReaction: a step must be an object (a table in Luau), got %s", type(entry)), level)
    end
    if rawget(entry, "op") ~= nil then
      error(RUNTIME_VALUE_ERROR, level)
    end
    local name = rawget(entry, "name")
    if name ~= nil then
      error(string.format(
        "defineReaction: reaction `%s` is a reaction handle, not a step; fire it with `fire(r)`",
        tostring(name)
      ), level)
    end
    if rawget(entry, "progress") ~= nil then
      error("defineReaction: a `progress` body is not a step", level)
    end
    if rawget(entry, "sequence") ~= nil then
      error("defineReaction: a nested `{ sequence }` is not a step; list its steps instead", level)
    end
    local mixed = mixedKey(entry)
    if mixed ~= nil then
      error(mixedError(mixed), level)
    end
    local primitive = rawget(entry, "primitive")
    if primitive == nil then
      local shape = listShape(entry)
      if shape == "holey" then
        error(MISSING_ENTRY_ERROR, level)
      elseif shape == nil then
        error("defineReaction: an entry with no `primitive` is not a step", level)
      end
      flattenSteps(entry, out, level + 1)
    elseif rawget(entry, "onComplete") ~= nil then
      error(string.format(
        "defineReaction: step `%s` carries `onComplete`, which only a body may",
        tostring(primitive)
      ), level)
    else
      table.insert(out, entry)
    end
  end
  return out
end

-- Lower an authored body to the manifest's shape: a list of steps, plain or
-- returned from the tracer, is a `{ sequence }` (an empty table an inert one),
-- and every `sequence` list is flattened. The second result is false for a
-- body of no known shape, returned as authored.
local function lowerBody(body: any, level: number): (any, boolean)
  if type(body) ~= "table" then
    return body, false
  end
  local mixed = mixedKey(body)
  if mixed ~= nil then
    error(mixedError(mixed), level)
  end
  local sequence = rawget(body, "sequence")
  if sequence ~= nil then
    if type(sequence) ~= "table" then
      error(SEQUENCE_SHAPE_ERROR, level)
    end
    local mixedInSequence = mixedKey(sequence)
    if mixedInSequence ~= nil then
      error(mixedError(mixedInSequence), level)
    end
    local shape = listShape(sequence)
    if shape == "holey" then
      error(MISSING_ENTRY_ERROR, level)
    elseif shape == nil then
      error(SEQUENCE_SHAPE_ERROR, level)
    end
    local lowered = table.clone(body)
    lowered.sequence = flattenSteps(sequence, {}, level + 1)
    return lowered, true
  end
  if rawget(body, "progress") ~= nil or rawget(body, "primitive") ~= nil then
    return body, true
  end
  local shape = listShape(body)
  if shape == "holey" then
    error(MISSING_ENTRY_ERROR, level)
  elseif shape == nil then
    return body, false
  end
  return { sequence = flattenSteps(body, {}, level + 1) }, true
end

--- Build a named reaction descriptor. Pure: returns a plain table, no FFI.
--- `descriptor` accepts one body shape: `progress`, `primitive`, `sequence`,
--- or a table of steps, which lowers to a `sequence`; nested tables flatten,
--- and an empty table `{}` is an inert reaction. A list entry that is not a
--- step raises. Any other body, a misspelled key included, passes through, and
--- the level load skips that one reaction with a warning. `name` is optional;
--- when omitted a deterministic, run-stable id is derived from the lowered
--- body. An explicit `name` wins over a body's own. Use explicit names when TS
--- and Luau scripts must agree. The returned handle can be passed to button
--- `onPress` or crossing `fire` entries.
function ReactionsSdk.defineReaction(
  nameOrBody: string | (ProgressReactionDescriptor | PrimitiveReactionDescriptor | SequenceReactionDescriptor | ReactionStepList) | ((CrossingParams) -> (ProgressReactionDescriptor | PrimitiveReactionDescriptor | SequenceReactionDescriptor | ReactionStepList)),
  descriptor: (ProgressReactionDescriptor | PrimitiveReactionDescriptor | SequenceReactionDescriptor | ReactionStepList | ((CrossingParams) -> (ProgressReactionDescriptor | PrimitiveReactionDescriptor | SequenceReactionDescriptor | ReactionStepList)))?
): NamedReactionDescriptor
  -- Level 3 from `lowerBody` is this function's caller: the author's line.
  local name: string
  local body: any
  local known: boolean
  if type(nameOrBody) == "string" then
    name = nameOrBody
    body, known = lowerBody(if type(descriptor) == "function" then (descriptor :: any)(DISPATCH_PARAMS) else descriptor, 3)
  else
    body, known = lowerBody(if type(nameOrBody) == "function" then (nameOrBody :: any)(DISPATCH_PARAMS) else nameOrBody, 3)
    name = autoReactionId(body)
  end
  local out: NamedReactionDescriptor = { name = name }
  if not known then
    -- The body's own raw keys, as TS spreads them, for the manifest parser to
    -- skip by name; raw, so a command target's strict metatable never fires.
    if type(body) == "table" then
      for key, value in next, body do
        if key ~= "name" then
          out[key] = value
        end
      end
    end
    return out
  end
  -- Copy fields from the body into the flat shape the deserializer reads. Only
  -- the fields present on each variant are set; unused fields are left absent.
  for _, key in ipairs({ "progress", "primitive", "kind", "tag", "target", "args", "onComplete", "sequence" }) do
    local value = rawget(body, key)
    if value ~= nil then
      out[key] = value
    end
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
