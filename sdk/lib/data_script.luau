--!strict
-- Data-script vocabulary: pure descriptor builders for `setupMod` and
-- `setupLevel`. `defineReaction` and `defineEntity` construct
-- typed plain tables that the engine deserializes from the script's
-- return value. They never call back into Rust -- the FFI boundary is the
-- `return` statement of the script entry point.
-- See: context/lib/scripting.md §2 (Data context lifecycle)
--
-- ---------------------------------------------------------------------
-- Canonical author example -- entity types registered via `setupMod`,
-- per-level reactions via `setupLevel`.
--
-- ```lua
-- function setupMod()
--   -- Entity types are returned from `setupMod` (definition context) and
--   -- survive level unload. Use `defineEntity` for a typed construction
--   -- site at the FFI boundary.
--   return {
--     entities = {
--       defineEntity({
--         classname = "light_dynamic",
--         components = { light = { color = { x = 1, y = 0.8, z = 0.6 }, intensity = 1.0, range = 6.0, is_dynamic = true } },
--       }),
--     },
--   }
-- end
--
-- function setupLevel(_ctx)
--   -- The manifest carries only per-level reactions.
--   return {
--     reactions = {
--       defineReaction("reactorWave1", {
--         progress = { tag = "reactorWave1Monsters", at = 1.0, fire = "wave1Complete" },
--       }),
--       defineReaction("wave1Complete", {
--         primitive = "moveGeometry",
--         tag = "reactorChambers",
--         onComplete = "wave2Revealed",
--       }),
--     },
--   }
-- end
-- ```
-- ---------------------------------------------------------------------

--- Progress-subscription reaction body: fires `fire` when entities tagged
--- `tag` cross the kill ratio `at` (0.0–1.0).
export type ProgressReactionDescriptor = {
  progress: { tag: string, at: number, fire: string },
}

--- Primitive reaction body: invokes the named Rust primitive. With `tag`, it
--- targets entities carrying that tag and mutates them. Without `tag`, it is a
--- system reaction (no entities) that enqueues a typed engine command —
--- `playSound`, `rumble`, `flashScreen`, the UI-stack reactions. `args`
--- carries the primitive's typed payload (e.g. `{ rate = 0 }` for
--- `setEmitterRate`).
export type PrimitiveReactionDescriptor = {
  primitive: string,
  tag: string?,
  args: { [string]: any }?,
  onComplete: string?,
}

--- Tween shape used by `setSpinRate`. Mirrors the Rust
--- `SpinAnimation` storage struct.
export type SpinAnimation = {
  duration: number,
  rate_curve: {number},
}

--- One step in a `sequence` reaction body. Sequence steps target a single
--- `EntityId`; tag-targeted primitives (`setEmitterRate`, `setSpinRate`)
--- belong on the `reactions:` -> `Primitive` path, not on `sequence`.
export type SetLightAnimationStep = {
  id: number,
  primitive: "setLightAnimation",
  args: any,
}

--- Fog sequence step shapes accepted by sequence reactions. Mirrors the
--- generated step types in `postretro.d.luau`; new sequenced fog
--- primitives extend both unions in lockstep.
--- `SetFogAnimationStep` is declared in `postretro.d.luau` (SDK lib block)
--- and included in `SequenceStep` below — no local redeclaration needed.
export type SetFogDensityStep = {
  id: number,
  primitive: "setFogDensity",
  args: { density: number },
}

export type SetFogScatterStep = {
  id: number,
  primitive: "setFogScatter",
  args: { scatter: number },
}

export type SetFogEdgeSoftnessStep = {
  id: number,
  primitive: "setFogEdgeSoftness",
  args: { edgeSoftness: number },
}

export type SetFogFalloffStep = {
  id: number,
  primitive: "setFogFalloff",
  args: { falloff: number },
}

export type SetFogParamsStep = {
  id: number,
  primitive: "setFogParams",
  args: { density: number?, scatter: number?, edgeSoftness: number?, falloff: number? },
}

-- `SetFogAnimationStep` is declared in `postretro.d.luau` (the SDK lib
-- block generated from the primitive registry) and is not redeclared here.
export type SequenceStep = SetLightAnimationStep | SetFogDensityStep | SetFogScatterStep | SetFogEdgeSoftnessStep | SetFogFalloffStep | SetFogParamsStep | SetFogAnimationStep

--- Descriptor produced by `defineReaction`. The `name` field is merged
--- into the descriptor at the top level so the Rust deserializer can read
--- both the event name and the descriptor body from a single flat table.
export type NamedReactionDescriptor = {
  name: string,
  -- progress shape
  progress: { tag: string, at: number, fire: string }?,
  -- primitive shape
  primitive: string?,
  tag: string?,
  args: { [string]: any }?,
  onComplete: string?,
  -- sequence shape
  sequence: {SequenceStep}?,
}

--- Bundle returned from `setupLevel`. The engine deserializes
--- this shape in one pass at level load and drops the data-script VM
--- context after the return.
--- `entities` is absent: entity-type registrations belong in the
--- `entities` field of `setupMod`'s return value, not here.
export type LevelManifest = {
  reactions: {NamedReactionDescriptor},
  -- State-crossing watchers (HUD dynamics). `CrossingDescriptor` is declared
  -- in postretro.d.luau (the generated SDK-lib type block). See
  -- `onStateCrossing`.
  crossings: {CrossingDescriptor}?,
  -- Per-level UI trees (name + `AnchoredTree` + `alwaysOn`). Optional; same shape
  -- as `ModManifest.uiTrees` but level-scoped (cleared on unload). Malformed
  -- entries are logged and skipped.
  uiTrees: {ModUiTree}?,
}

export type StoreSlotSchema = { type: string, readonly: boolean?, [string]: any }
export type StoreDeclaration = {
  namespace: string,
  schema: { [string]: StoreSlotSchema },
}
export type ReadonlyStateRef<T> = { slot: string, __stateRefValueBrand: T? }
export type WritableStateRef<T> = ReadonlyStateRef<T> & { __writableStateRefBrand: T }
export type StateRef<T> = ReadonlyStateRef<T> | WritableStateRef<T>
export type StoreDefinition = {
  declaration: StoreDeclaration,
  state: { [string]: StateRef<any> },
}

local DataScriptSdk = {}

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

--- Build a named reaction descriptor. Pure: returns a plain table, does
--- not register anything in the engine. The engine consumes the table
--- when `setupLevel` returns. `name` is optional: when omitted a deterministic,
--- run-stable id is derived from the body (FNV-1a over an order-stable
--- serialization), so re-running registration yields the same auto-id.
function DataScriptSdk.defineReaction(
  nameOrBody: string | (ProgressReactionDescriptor | PrimitiveReactionDescriptor | SequenceReactionDescriptor),
  descriptor: (ProgressReactionDescriptor | PrimitiveReactionDescriptor | SequenceReactionDescriptor)?
): NamedReactionDescriptor
  local name: string
  local body: any
  if type(nameOrBody) == "string" then
    name = nameOrBody
    body = descriptor
  else
    body = nameOrBody
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
  if d.tag ~= nil then
    out.tag = d.tag
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

--- Identity builder for entity type descriptors returned from `setupMod`.
--- Pure: returns the descriptor as-is. Its sole purpose is to give authors
--- a typed construction site at the FFI boundary.
function DataScriptSdk.defineEntity(descriptor: EntityTypeDescriptor): EntityTypeDescriptor
  return descriptor
end

--- Identity builder for the object returned from `setupMod`.
--- Pure: returns the manifest as-is. The engine consumes it only at the
--- setup return boundary.
function DataScriptSdk.defineMod(config: ModManifest): ModManifest
  return config
end

--- Identity builder for a mod map catalog. Pure: returns the array as-is.
function DataScriptSdk.defineMapCatalog(entries: {ModMapEntry}): {ModMapEntry}
  return entries
end

local MAGIC_SCHEMA_KEYS = {
  __proto__ = true,
  constructor = true,
  prototype = true,
}

local function schemaPath(parent: string, key: any): string
  if type(key) == "string" then
    if string.match(key, "^[%a_][%w_]*$") ~= nil then
      return parent .. "." .. key
    end
    return parent .. "[" .. string.format("%q", key) .. "]"
  end
  return parent .. "[" .. tostring(key) .. "]"
end

local function rejectMagicSchemaKey(key: any, path: string)
  if type(key) == "string" and MAGIC_SCHEMA_KEYS[key] then
    error("defineStore schema key " .. schemaPath(path, key) .. " is reserved", 3)
  end
end

local function cloneAndFreeze(value: any, seenOrNil: any?, visitingOrNil: any?, pathOrNil: string?): any
  if type(value) ~= "table" then
    return value
  end
  local seen = seenOrNil or {}
  local visiting = visitingOrNil or {}
  local path = pathOrNil or "schema"
  if visiting[value] then
    error("defineStore schema contains a cycle at " .. path, 2)
  end
  local cached = seen[value]
  if cached ~= nil then
    return cached
  end
  visiting[value] = true
  local clone = {}
  seen[value] = clone
  for key, child in pairs(value) do
    rejectMagicSchemaKey(key, path)
    clone[key] = cloneAndFreeze(child, seen, visiting, schemaPath(path, key))
  end
  visiting[value] = nil
  return table.freeze(clone)
end

--- Pure state-store builder. The engine consumes `declaration` only when it is
--- returned from `setupMod().stores`; unreturned declarations disappear with
--- the setup VM.
function DataScriptSdk.defineStore(namespace: string, schema: { [string]: StoreSlotSchema }): StoreDefinition
  local frozenSchema = cloneAndFreeze(schema)
  local state = {}
  for slotName in pairs(frozenSchema) do
    state[slotName] = table.freeze({ slot = namespace .. "." .. slotName })
  end
  return table.freeze({
    declaration = table.freeze({
      namespace = namespace,
      schema = frozenSchema,
    }),
    state = table.freeze(state),
  })
end

return DataScriptSdk
