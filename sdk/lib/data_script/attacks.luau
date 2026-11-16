--!nocheck
-- Attack and stance declarations: `defineAttack` and `defineStance`. A handle
-- carries its whole declaration, so a behavior that references it needs no
-- separate registration; load-time lowering in the engine resolves handles to
-- the attack names the runtime descriptor uses.
-- Part chunk of `data_script.luau`: evaluated before it, its builders merged
-- into the returned table.
-- See: context/lib/scripting.md §2 (Attack and stance declarations)

local AttacksSdk = {}

local ATTACK_NAME_DIAGNOSTIC =
  "defineAttack needs its name: Luau passes it explicitly, as defineAttack(name, { ... })"
local STANCE_NAME_DIAGNOSTIC =
  "defineStance needs its name: Luau passes it explicitly, as defineStance(name, { ... })"

-- A handle is its declaration plus `kind` and `name`, frozen. Nested handles
-- (an attack's stances, its draws' tokens) stay the frozen handles they are.
local function handle(kind: string, name: string, declaration: any): any
  local fields = table.clone(declaration or {})
  setmetatable(fields, nil)
  fields.kind = kind
  fields.name = name
  return table.freeze(fields)
end

--- Declare an attack: its engine fields (`damage`, `maxRange`, `cooldownMs`,
--- `draws`, ... or `weapon`) plus the stance it begins and ends in and its
--- `release` and `recover`. Luau passes the name explicitly. Reference the
--- handle wherever an attack is meant: `action = { attack = slam }`,
--- `attackTurn`, a behavior's `attacks` list, `attackTurnFree`,
--- `attackTurnHeld`.
function AttacksSdk.defineAttack(...: any): Attack
  local name, declaration = ...
  if type(name) ~= "string" then
    error(ATTACK_NAME_DIAGNOSTIC, 2)
  end
  return handle("attack", name, declaration)
end

--- Declare a stance: a pose an enemy rests in before an attack that names it
--- as its `beginningStance`. Luau passes the name explicitly.
function AttacksSdk.defineStance(...: any): Stance
  local name, options = ...
  if type(name) ~= "string" then
    error(STANCE_NAME_DIAGNOSTIC, 2)
  end
  return handle("stance", name, options)
end

return {
  builders = AttacksSdk,
}
