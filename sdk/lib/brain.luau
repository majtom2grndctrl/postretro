--!strict
-- Behavior-graph guard and candidate inputs: pre-wrapped IR input leaves for
-- the fixed brain and candidate namespaces, plus the `@state.<name>` leaf
-- builder.
--
-- Pure SDK sugar: a leaf is plain data (`{ op = "input", name = ... }`), the
-- same value `runtime.read(name)` returns. Constructed literally here so this
-- module has no prelude-evaluation-order dependency on the `runtime` global.
-- Frozen because the `runtime` builders never mutate their operands, so one
-- shared leaf per input is safe across every guard that reads it.
--
-- SYNC OBLIGATION: `brain` mirrors the `BRAIN_INPUTS` table in
-- `crates/foundation/src/brain.rs`; `candidate` mirrors `CANDIDATE_INPUTS` in
-- `crates/foundation/src/candidate.rs`. `brain.ts` carries both sets. Adding an
-- input means editing both preludes; the SDK-helper drift tests in
-- `crates/scripting-core/src/data_descriptors/tests/behavior.rs` fail until
-- they agree.
-- See: context/lib/scripting.md §11 · context/lib/entity_model.md §4

local function input(name: string): RuntimeRead
  return table.freeze({ op = "input", name = name })
end

local BrainSdk = {}

--- Pre-wrapped guard input leaves for the fixed `@brain.*` namespace:
--- `hasTarget` (boolean; the only authoritative target-presence test),
--- `targetDistance` (number; `1e9` with no target),
--- `timeInStateMs`, `attackCooldownMs`, `acquisitionDue` (boolean), `health`,
--- `maxHealth`, `targetHealth` and `targetMaxHealth` (zero with no target or
--- health component), `targetDied` (boolean; false with no target), and
--- `distanceFromAnchor` (number; the enemy's XZ distance from its spawn-time
--- home anchor, including untargeted), and `targetHostile` (boolean; true
--- when the selected target's faction differs, false with no target).
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
  distanceFromAnchor = input("@brain.distanceFromAnchor"),
  targetHostile = input("@brain.targetHostile"),
})

--- Facts about one offered target, evaluated during acquisition.
BrainSdk.candidate = table.freeze({
  distance = input("@candidate.distance"),
  health = input("@candidate.health"),
  maxHealth = input("@candidate.maxHealth"),
  died = input("@candidate.died"),
})

--- Read a per-entity state field as a guard input: `state("staggered")` is the
--- `@state.staggered` leaf. Unset fields read as `0`. Impact policies and
--- reactions write these; guards only read them.
function BrainSdk.state(name: string): RuntimeRead
  return input("@state." .. name)
end

return BrainSdk
