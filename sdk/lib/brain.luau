--!strict
-- Behavior-graph guard and candidate inputs: pre-wrapped IR input leaves for
-- the fixed brain and candidate namespaces, plus the `@state.<name>` leaf
-- builder.
--
-- Pure SDK sugar: a leaf is plain data (`{ op = "input", name = ... }`), the
-- same value `runtime.read(name)` returns. `runtime.read` attaches the fluent
-- guard metatable before this module freezes the reusable leaf.
-- Frozen because the `runtime` builders never mutate their operands, so one
-- shared leaf per input is safe across every guard that reads it.
--
-- SYNC OBLIGATION: `brain` mirrors the `BRAIN_INPUTS` table in
-- `crates/foundation/src/brain.rs`; `candidate` mirrors `CANDIDATE_INPUTS` in
-- `crates/foundation/src/candidate.rs`. `brain.ts` carries both sets. Adding an
-- input means editing both preludes and both typedef templates;
-- `brain_input_typedefs_match_the_foundation_table` (in
-- `crates/sim/src/scripting/typedef/tests/surface.rs`) fails until the
-- templates agree with the table.
-- See: context/lib/scripting.md §11 · context/lib/entity_model.md §4

local function input(name: string): RuntimeGuardNode
  return table.freeze(runtime.read(name)) :: RuntimeGuardNode
end

local BrainSdk = {}

--- Pre-wrapped guard input leaves for the fixed `@brain.*` namespace:
--- `hasTarget` (boolean; the only authoritative target-presence test),
--- `targetDistance` (number; `1e9` with no target),
--- `timeInActivityMs` (number; scope-relative to the activity whose rows are
--- being evaluated), `attackCooldownMs`, `acquisitionDue` (boolean), `health`,
--- `maxHealth`, `targetHealth` and `targetMaxHealth` (zero with no target or
--- health component), `targetDied` (boolean; false with no target), and
--- `distanceFromAnchor` (number; the enemy's XZ distance from its spawn-time
--- home anchor, including untargeted), and `targetHostile` (boolean; true
--- when directional sentiment toward the selected target is negative, false
--- with no target), and
--- `targetReachable` (boolean; false with no target or no navmesh;
--- the pathfinder's cached verdict rather than ground-truth reachability), and
--- `attacksFiredInActivity` (number; successful fires since the current
--- activity entered, visible to guards on the next refresh), and
--- `targetVisible` (boolean; the shared debounced sightline verdict, which
--- static geometry and movers at their current pose block, false with no
--- target), and `timeSinceDamageMs` (number; `1e9`
--- before the first hit, then zero at a landed hit and monotonically aged back
--- to that sentinel by AI ticks), and `timeSinceTargetVisible` (number; `1e9`
--- before the first sighting, then zero while a selected target is visible and
--- monotonically aged back to that sentinel), and `distanceToLastKnown`
--- (number; XZ distance to the remembered spot or `1e9` without memory — pair
--- bare `gt`/`ge` guards with recent sight, or with both recent damage and
--- `damageSourceKnown`), and
--- `damageBearing` (number; signed XZ yaw from visual `+Z` toward the most
--- recent damage attacker, zero ahead and opposite signs on each side), and
--- `damageSourceKnown` (boolean; true only when the latest damaging hit
--- supplied an attacker world position; pair it with damage recency),
--- `hasAttackTurn` (boolean; an attack turn is held, for any attack, as of the
--- previous tick), and `animationDone` (boolean; true from the tick after the
--- selected non-looping animation's clip reaches its duration, false on a
--- looping state, true for a non-looping state whose clip did not resolve; entity-wide,
--- not scope-relative), and `activityRoll` (number; uniform in [0, 1), drawn
--- when the currently-evaluated activity was entered and fixed until it is
--- left; scope-relative like `timeInActivityMs`), and `damageInActivity`
--- (number; points drained from defense layers and health since the
--- currently-evaluated activity was entered, by every damage producer,
--- `applyDamage` included; scope-relative like `timeInActivityMs`, resetting
--- to 0 when that level is entered), and `awareness` (number; highest
--- awareness of any candidate, 0 to 1, 1 once it has a target; a guard on a
--- decide reads the value that decide's acquisition check produced), and
--- `attacksHeldInActivity` (number; shots the gun's near-path test held since
--- the current activity entered, visible to guards on the next refresh;
--- host-only).
BrainSdk.brain = table.freeze({
  hasTarget = input("@brain.hasTarget"),
  targetDistance = input("@brain.targetDistance"),
  timeInActivityMs = input("@brain.timeInActivityMs"),
  attackCooldownMs = input("@brain.attackCooldownMs"),
  acquisitionDue = input("@brain.acquisitionDue"),
  health = input("@brain.health"),
  maxHealth = input("@brain.maxHealth"),
  targetHealth = input("@brain.targetHealth"),
  targetMaxHealth = input("@brain.targetMaxHealth"),
  targetDied = input("@brain.targetDied"),
  distanceFromAnchor = input("@brain.distanceFromAnchor"),
  targetHostile = input("@brain.targetHostile"),
  targetReachable = input("@brain.targetReachable"),
  attacksFiredInActivity = input("@brain.attacksFiredInActivity"),
  targetVisible = input("@brain.targetVisible"),
  timeSinceDamageMs = input("@brain.timeSinceDamageMs"),
  timeSinceTargetVisible = input("@brain.timeSinceTargetVisible"),
  distanceToLastKnown = input("@brain.distanceToLastKnown"),
  damageBearing = input("@brain.damageBearing"),
  damageSourceKnown = input("@brain.damageSourceKnown"),
  hasAttackTurn = input("@brain.hasAttackTurn"),
  animationDone = input("@brain.animationDone"),
  activityRoll = input("@brain.activityRoll"),
  damageInActivity = input("@brain.damageInActivity"),
  awareness = input("@brain.awareness"),
  attacksHeldInActivity = input("@brain.attacksHeldInActivity"),
})

--- Facts about one offered target, evaluated during acquisition.
BrainSdk.candidate = table.freeze({
  distance = input("@candidate.distance"),
  health = input("@candidate.health"),
  maxHealth = input("@candidate.maxHealth"),
  died = input("@candidate.died"),
  sentiment = input("@candidate.sentiment"),
  damageDealtToMe = input("@candidate.damageDealtToMe"),
  timeSinceDamageFromCandidate = input("@candidate.timeSinceDamageFromCandidate"),
  tolerance = input("@candidate.tolerance"),
})

--- Read a per-entity state field as a guard input: `state("staggered")` is the
--- `@state.staggered` leaf. Unset fields read as `0`. Impact policies and
--- reactions write these; guards only read them.
function BrainSdk.state(name: string): RuntimeGuardNode
  return input("@state." .. name)
end

-- The `@defense.` input a layer handle reads; a bad reference (a bare string,
-- a damage-type handle) names no layer, so the read fails load.
local function defenseInput(layer: any, capacity: boolean): string
  local name = "@invalid"
  if type(layer) == "table" and rawget(layer, "kind") == "defenseLayer" and type(rawget(layer, "name")) == "string" then
    name = rawget(layer, "name")
  end
  return "@defense." .. name .. (if capacity then ".capacity" else "")
end

--- Read the brain entity's own defense layer as guard inputs, as `state(name)`
--- reads its own state: `defense(armor).current:le(0)`. An entity without the
--- layer reads `0`.
function BrainSdk.defense(layer: any): { current: RuntimeGuardNode, capacity: RuntimeGuardNode }
  return table.freeze({
    current = input(defenseInput(layer, false)),
    capacity = input(defenseInput(layer, true)),
  })
end

--- `true` while the brain entity carries a live instance of `status`, from
--- any applier. A status `defineMod` does not register fails load.
function BrainSdk.hasStatus(status: any): RuntimeGuardNode
  local name = "@invalid"
  if type(status) == "table" and rawget(status, "kind") == "status" and type(rawget(status, "name")) == "string" then
    name = rawget(status, "name")
  end
  return input("@status." .. name)
end

-- The attack name a turn fact reads. Anything but an attack handle (a bare
-- string, nil, a stance) raises at the call, naming the fact.
local function attackHandleName(fact: string, attack: any): string
  if type(attack) == "table" and rawget(attack, "kind") == "attack" and type(rawget(attack, "name")) == "string" then
    return rawget(attack, "name")
  end
  local found
  if type(attack) == "string" then
    found = 'the bare string "' .. attack .. '"'
  elseif attack == nil then
    found = "nil"
  elseif type(attack) == "table" then
    found = 'a "' .. tostring(rawget(attack, "kind")) .. '" handle'
  else
    found = "a " .. type(attack)
  end
  error(fact .. "(attack): expected an attack handle from defineAttack, got " .. found, 3)
end

--- `true` while the enemy holds a turn for `attack`, one of its archetype's
--- attacks, as of the previous tick. Naming another attack fails load. Reading
--- it registers no attack.
function BrainSdk.attackTurnHeld(attack: Attack): RuntimeGuardNode
  return input("@attackTurnHeld." .. attackHandleName("attackTurnHeld", attack))
end

--- `true` when a request for `attack`, one of its archetype's attacks, would be
--- granted now against the previous tick's counts and reservations, leaving out
--- the enemy's own pending request. False with no target, and for an attack
--- sharing a token with a turn the enemy holds. Naming another attack fails
--- load. Reading it registers no attack.
function BrainSdk.attackTurnFree(attack: Attack): RuntimeGuardNode
  return input("@attackTurnFree." .. attackHandleName("attackTurnFree", attack))
end

return BrainSdk
