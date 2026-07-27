--!strict
-- Behavior-graph guard inputs: pre-wrapped IR input leaves for the fixed
-- `@brain.*` namespace, plus the `@state.<name>` leaf builder.
--
-- Pure SDK sugar: a leaf is plain data (`{ op = "input", name = ... }`), the
-- same value `runtime.read(name)` returns. Constructed literally here so this
-- module has no prelude-evaluation-order dependency on the `runtime` global.
-- Frozen because the `runtime` builders never mutate their operands, so one
-- shared leaf per input is safe across every guard that reads it.
--
-- SYNC OBLIGATION: `brain`'s keys are the `BRAIN_INPUTS` table in
-- `crates/foundation/src/brain.rs`, one key per entry, named by stripping the
-- `@brain.` prefix. `brain.ts` carries the same set. Adding a brain input means
-- editing all three; `brain_sdk_helpers_cover_every_brain_input` (in
-- `crates/scripting-core/src/data_descriptors/tests/behavior.rs`) fails until
-- they agree.
-- See: context/lib/scripting.md §11 · context/lib/entity_model.md §4

local function input(name: string): RuntimeRead
  return table.freeze({ op = "input", name = name })
end

local BrainSdk = {}

--- Pre-wrapped guard input leaves for the fixed `@brain.*` namespace:
--- `hasTarget` (boolean), `targetDistance` (number; `1e9` with no target),
--- `timeInStateMs`, `attackCooldownMs`, `acquisitionDue` (boolean), `health`,
--- `maxHealth`, `targetHealth`, `targetMaxHealth`, and `targetDied` (boolean).
BrainSdk.brain = table.freeze({
  hasTarget = input("@brain.hasTarget"),
  targetDistance = input("@brain.targetDistance"),
  timeInStateMs = input("@brain.timeInStateMs"),
  attackCooldownMs = input("@brain.attackCooldownMs"),
  acquisitionDue = input("@brain.acquisitionDue"),
  health = input("@brain.health"),
  maxHealth = input("@brain.maxHealth"),
  targetHealth = input("@brain.targetHealth"),
  targetMaxHealth = input("@brain.targetMaxHealth"),
  targetDied = input("@brain.targetDied"),
})

--- Read a per-entity state field as a guard input: `state("staggered")` is the
--- `@state.staggered` leaf. Unset fields read as `0`. Impact policies and
--- reactions write these; guards only read them.
function BrainSdk.state(name: string): RuntimeRead
  return input("@state." .. name)
end

return BrainSdk
