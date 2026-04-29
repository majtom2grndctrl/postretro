--!strict
-- Data-script vocabulary: pure descriptor builders for `registerLevelManifest`.
-- `registerReaction` and `registerEntities` construct typed plain tables that
-- the engine deserializes from the manifest's return value. They never call
-- back into Rust -- the FFI boundary is the `return` statement of
-- `registerLevelManifest`.
-- See: context/lib/scripting.md §2 (Data context lifecycle)
--
-- ---------------------------------------------------------------------
-- Canonical author example -- wave-on-clear scripted reveal.
--
-- ```lua
-- function registerLevelManifest(_ctx)
--   return {
--     entities = registerEntities({ Grunt, HeavyGunner }),
--     reactions = {
--       registerReaction("reactorWave1", {
--         progress = { tag = "reactorWave1Monsters", at = 1.0, fire = "wave1Complete" },
--       }),
--       registerReaction("wave1Complete", {
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

--- Primitive reaction body: invokes the named Rust primitive on entities
--- tagged `tag`, optionally firing `onComplete` when it finishes.
export type PrimitiveReactionDescriptor = {
  primitive: string,
  tag: string,
  onComplete: string?,
}

--- Descriptor produced by `registerReaction`. The `name` field is merged
--- into the descriptor at the top level so the Rust deserializer can read
--- both the event name and the descriptor body from a single flat table.
export type NamedReactionDescriptor = {
  name: string,
  -- progress shape
  progress: { tag: string, at: number, fire: string }?,
  -- primitive shape
  primitive: string?,
  tag: string?,
  onComplete: string?,
}

--- Descriptor produced by `registerEntities` -- one entry per registered class.
export type EntityTypeDescriptor = { classname: string }

--- Bundle returned from `registerLevelManifest`. The engine deserializes
--- this shape in one pass at level load and drops the data-script VM
--- context after the return.
export type LevelManifest = {
  entities: {EntityTypeDescriptor},
  reactions: {NamedReactionDescriptor},
}

local DataScriptSdk = {}

--- Build a named reaction descriptor. Pure: returns a plain table, does
--- not register anything in the engine. The engine consumes the table
--- when `registerLevelManifest` returns.
function DataScriptSdk.registerReaction(
  name: string,
  descriptor: ProgressReactionDescriptor | PrimitiveReactionDescriptor
): NamedReactionDescriptor
  local out: NamedReactionDescriptor = { name = name }
  -- Copy fields from the descriptor into the flat shape the deserializer
  -- reads. Only the fields present on each variant are set; unused fields
  -- are left absent.
  local d = descriptor :: any
  if d.progress ~= nil then
    out.progress = d.progress
  end
  if d.primitive ~= nil then
    out.primitive = d.primitive
  end
  if d.tag ~= nil then
    out.tag = d.tag
  end
  if d.onComplete ~= nil then
    out.onComplete = d.onComplete
  end
  return out
end

--- Build the entity-type descriptor list for `LevelManifest.entities`.
--- Each input class is reduced to its `classname` -- that's the only
--- field the engine reads at registration time. Pure: returns a fresh
--- table.
function DataScriptSdk.registerEntities(
  types: {{ classname: string }}
): {EntityTypeDescriptor}
  local out: {EntityTypeDescriptor} = {}
  for i, t in ipairs(types) do
    out[i] = { classname = t.classname }
  end
  return out
end

return DataScriptSdk
