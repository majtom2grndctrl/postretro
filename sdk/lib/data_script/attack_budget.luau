--!nocheck
-- Attack-token declarations: `defineAttackToken`, the SDK's `defaultToken`, and
-- `token:with(...)` references. Pure data; a token counts only once
-- `defineMod({ attackTokens = { ... } })` registers it.
-- Part chunk of `data_script.luau`: evaluated first, its builders merged into
-- the returned table and `attackTokenDeclarationOf` read by `defineMod`.
-- See: context/lib/scripting.md §2 (Attack token declarations)

local AttackBudgetSdk = {}

local NAME_DIAGNOSTIC =
  "defineAttackToken needs its name: Luau passes it explicitly, as defineAttackToken(name, { limit, refreshMs })"

-- Declarations `defineMod({ attackTokens })` resolves each handle through.
local attackTokenDeclarations = setmetatable({}, { __mode = "k" })

-- A handle crosses as `{ kind, name }`; `with` lives on the metatable, off the wire.
local function attackTokenHandle(name: string): AttackToken
  local handle
  local methods = {
    with = function(_self, fields: AttackTokenFields): AttackTokenRef
      -- `melee.with({ ... })` binds the fields table to `_self` and leaves
      -- `fields` nil: a silent cost-1 draw. Reject it rather than guess.
      if fields == nil and type(_self) == "table" and _self ~= handle then
        error("attack token: call `with` as a method, melee:with({ ... }), not melee.with({ ... })", 2)
      end
      local reference = table.clone(fields or {})
      reference.token = handle
      return table.freeze(reference)
    end,
  }
  handle = table.freeze(setmetatable({ kind = "attackToken", name = name }, { __index = methods }))
  return handle
end

--- The SDK's attack token at the default budget: what an attack without
--- `draws` draws. Every mod has it and never lists it in `attackTokens`; a
--- profile may override it.
AttackBudgetSdk.defaultToken = attackTokenHandle("default")

--- Declare an attack token by its default budget: `limit` (default 3) is how
--- much of it may be drawn on one target at once, `refreshMs` (default 0) how
--- long a spent draw stays counted after its turn ends. Luau passes the name
--- explicitly. Register it with `defineMod({ attackTokens = { token } })`.
function AttackBudgetSdk.defineAttackToken(...: any): AttackToken
  local name, budget = ...
  if type(name) ~= "string" then
    error(NAME_DIAGNOSTIC, 2)
  end
  local handle = attackTokenHandle(name)
  attackTokenDeclarations[handle] =
    table.freeze({ kind = "attackToken", name = name, budget = table.clone(budget or {}) })
  return handle
end

-- The declaration an `attackTokens` entry carries across the manifest. A bare
-- string, a handle of another kind or `defaultToken` passes through, and the
-- engine rejects it naming the list entry.
local function attackTokenDeclarationOf(entry: any): any
  local declaration = if type(entry) == "table" then attackTokenDeclarations[entry] else nil
  return if declaration ~= nil then declaration else entry
end

return {
  builders = AttackBudgetSdk,
  attackTokenDeclarationOf = attackTokenDeclarationOf,
}
