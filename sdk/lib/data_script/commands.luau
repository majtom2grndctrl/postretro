--!nocheck
-- Command targets: the `npcs` and `players` groups and the subject tokens
-- (`on.activators`, `on.trigger`, `on.player`). Each carries exactly the verbs
-- its kind supports, called with colon syntax; a verb builds the closed wire
-- descriptor.
-- Part chunk of `data_script.luau`: evaluated after `player_events.luau`
-- (whose `playerEvent` builds `players():on`), published through the
-- temporary `__postretroDataScriptParts` bridge (`reactions.luau` reads the
-- subject tokens from it), and merged into its returned builder table. Type
-- names refer to the declarations in `data_script.luau`.
-- See: context/lib/scripting.md §7 (SDK library globals), §12

local CommandsSdk = {}

local playerEvent = __postretroDataScriptParts.playerEvents.playerEvent

-- A command target is opaque: an empty frozen table whose metatable serves its
-- verbs. Indexing anything else, `#target`, `for _ in target`, or a write raises
-- an error naming the call that built the target, so a group never reads as 0
-- or nil. `pairs(target)` cannot raise — Luau's `pairs` ignores metatables —
-- but it walks the empty table and yields no member.
-- Each verb checks its receiver, so `target.verb(x)` (a dot call, which passes
-- `x` as the receiver) raises instead of building a command from shifted args.
-- `wire`, when given, is served as `__wire` so an engine state ref's
-- `byPlayer`, built before this chunk runs, can recognize the token.
local function opaqueTarget(call: string, verbs: { [string]: any }, wire: string?)
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
  local target = {}
  local bound = {}
  for name, verb in pairs(verbs) do
    bound[name] = function(self, ...)
      if not rawequal(self, target) then
        error(
          string.format(
            "%s.%s was called with `.`; call it with `:`, as `%s:%s(...)`",
            call,
            name,
            call,
            name
          ),
          2
        )
      end
      return verb(self, ...)
    end
  end
  -- Both the target and its metatable are frozen, so no write or metatable
  -- swap can open it up (a protected `__metatable` would refuse the freeze).
  return table.freeze(setmetatable(target, table.freeze({
    __index = function(_target, key)
      local verb = bound[key]
      if verb ~= nil then
        return verb
      end
      if key == "__wire" and wire ~= nil then
        return wire
      end
      if type(key) == "string" then
        error(misuse(string.format("has no verb or field `%s`", key)), 2)
      end
      error(misuse(string.format("cannot be indexed by %s", tostring(key))), 2)
    end,
    __len = function()
      error(misuse("has no length"), 2)
    end,
    __iter = function()
      error(misuse("cannot be iterated"), 2)
    end,
    __newindex = function()
      error(misuse("cannot be written"), 2)
    end,
  })))
end

-- The four player-facing verbs, shared by `players()`, `on.activators` and
-- `on.player`; only the addressing each stamps differs.
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
--- effect. Opaque: no members, no length. `players():on(edge, fire, options?)`
--- builds a player event for a manifest's `playerEvents`.
function CommandsSdk.players(): PlayerGroup
  local verbs = playerVerbs(groupCommand("player", nil))
  verbs.on = function(_self, edge, fire, options)
    return playerEvent(edge, fire, options)
  end
  return opaqueTarget("players()", verbs)
end

-- Subject tokens lower to one descriptor, `{ primitive, target, args }`, used
-- unchanged as a reaction body or a sequence entry.
local function tokenCommand(target: string)
  return function(primitive: string, args)
    return { primitive = primitive, target = target, args = args }
  end
end

local ACTIVATORS_TARGET = opaqueTarget("on.activators", playerVerbs(tokenCommand("@activators")))
local PLAYER_TARGET = opaqueTarget("on.player", playerVerbs(tokenCommand("@player")), "@player")

local triggerCommand = tokenCommand("@trigger")
local TRIGGER_TARGET = opaqueTarget("on.trigger", {
  arm = function(_self)
    return triggerCommand("armTrigger", {})
  end,
  disarm = function(_self)
    return triggerCommand("disarmTrigger", {})
  end,
})

return {
  builders = CommandsSdk,
  ACTIVATORS_TARGET = ACTIVATORS_TARGET,
  PLAYER_TARGET = PLAYER_TARGET,
  TRIGGER_TARGET = TRIGGER_TARGET,
}
