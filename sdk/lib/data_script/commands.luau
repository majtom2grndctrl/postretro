--!nocheck
-- Command targets: the `npcs` and `players` groups and the trigger-event
-- subject tokens (`on.activators`, `on.trigger`). Each carries exactly the
-- verbs its kind supports, called with colon syntax; a verb builds the closed
-- wire descriptor.
-- Part chunk of `data_script.luau`: evaluated first, published through the
-- temporary `__postretroDataScriptParts` bridge (`reactions.luau` reads the
-- subject tokens from it), and merged into its returned builder table. Type
-- names refer to the declarations in `data_script.luau`.
-- See: context/lib/scripting.md §7 (SDK library globals), §12

local CommandsSdk = {}

-- A command target is opaque: an empty frozen table whose metatable serves its
-- verbs. Indexing anything else, `#target`, or a write raises an error naming
-- the call that built the target, so a group never reads as 0 or nil.
local function opaqueTarget(call: string, verbs: { [string]: any })
  local names = {}
  for name in pairs(verbs) do
    table.insert(names, name)
  end
  table.sort(names)
  local verbList = table.concat(names, ", ")
  local function misuse(what: string): string
    return string.format(
      "%s %s: it is opaque, with no members and no length; its verbs are %s",
      call,
      what,
      verbList
    )
  end
  -- Both the target and its metatable are frozen, so no write or metatable
  -- swap can open it up (a protected `__metatable` would refuse the freeze).
  return table.freeze(setmetatable({}, table.freeze({
    __index = function(_target, key)
      local verb = verbs[key]
      if verb ~= nil then
        return verb
      end
      if type(key) == "string" then
        error(misuse(string.format("has no verb or field `%s`", key)), 2)
      end
      error(misuse(string.format("cannot be indexed by %s", tostring(key))), 2)
    end,
    __len = function()
      error(misuse("has no length"), 2)
    end,
    __newindex = function()
      error(misuse("cannot be written"), 2)
    end,
  })))
end

-- The four player-facing verbs, shared by `players()` and `on.activators`;
-- only the addressing each stamps differs.
local function playerVerbs(make)
  return {
    damage = function(_self, amount: number)
      return make("applyDamage", { amount = amount })
    end,
    grantHealth = function(_self, amount: number)
      return make("grantHealth", { amount = amount })
    end,
    grantAmmo = function(_self, ammoType: string, amount: number)
      return make("grantAmmo", { type = ammoType, amount = amount })
    end,
    addSlot = function(_self, slot, delta: number)
      return make("addSlot", { slot = slot.slot, delta = delta })
    end,
  }
end

local function groupCommand(kind: string, tag: string?)
  return function(primitive: string, args)
    local out = { primitive = primitive, kind = kind, args = args }
    if tag ~= nil then
      out.tag = tag
    end
    return out
  end
end

--- Address NPCs, optionally narrowed to those carrying `tag`. Resolved when
--- each command takes effect. Opaque: no members, no length.
function CommandsSdk.npcs(filter: NpcGroupFilter?): NpcGroup
  local tag = if filter ~= nil then filter.tag else nil
  local call = if tag ~= nil then string.format("npcs({ tag = %q })", tag) else "npcs()"
  local make = groupCommand("npc", tag)
  return opaqueTarget(call, {
    update = function(_self, fields: NpcStateUpdateArgs)
      return make("updateNpcState", fields)
    end,
    damage = function(_self, amount: number)
      return make("applyDamage", { amount = amount })
    end,
  })
end

--- Address every seat-bound player pawn. Resolved when each command takes
--- effect. Opaque: no members, no length.
function CommandsSdk.players(): PlayerGroup
  return opaqueTarget("players()", playerVerbs(groupCommand("player", nil)))
end

-- Subject tokens lower to the existing sentinel wire: `target = "@activators"`
-- on a primitive body, `id = "@trigger"` on a sequence step.
local ACTIVATORS_TARGET = opaqueTarget(
  "on.activators",
  playerVerbs(function(primitive: string, args)
    return { primitive = primitive, target = "@activators", args = args }
  end)
)

local TRIGGER_TARGET = opaqueTarget("on.trigger", {
  arm = function(_self)
    return { { id = "@trigger", primitive = "armTrigger", args = {} } }
  end,
  disarm = function(_self)
    return { { id = "@trigger", primitive = "disarmTrigger", args = {} } }
  end,
})

return {
  builders = CommandsSdk,
  ACTIVATORS_TARGET = ACTIVATORS_TARGET,
  TRIGGER_TARGET = TRIGGER_TARGET,
}
