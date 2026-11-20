--!nocheck
-- Impact-policy builders: the `impact` scope a `defineImpactEvent` build
-- receives, its closed effects, gated groups and store-write helpers.
-- Part chunk of `data_script.luau`: evaluated before it, its builders merged
-- into the returned table and `BINDING_NAME_SUGAR_DIAGNOSTIC` read by it.
-- See: context/lib/scripting.md §2 (Impact-policy evaluation), §11

local ImpactEventsSdk = {}

local expressionRefs = __postretroExpressionRefs
local numberNode = expressionRefs.numberNode
local boolNode = expressionRefs.boolNode
local numberRef = expressionRefs.numberRef
local boolRef = expressionRefs.boolRef

-- Policy arrays keep their sequence shape across serde even when empty, as
-- `data_script.luau`'s descriptor arrays do.
local DESCRIPTOR_ARRAY_METATABLE = rawget(_G, "__postretroArrayMetatable")

local function descriptorArray(values: any): any
  if DESCRIPTOR_ARRAY_METATABLE == nil then
    return values
  end
  return setmetatable(values, DESCRIPTOR_ARRAY_METATABLE)
end

--- Test the evaluated player in player events, or the local player in crossings.
function ImpactEventsSdk.inLifeState(name: string): BoolRef
  return boolRef({ op = "input", name = "player.inLifeState:" .. name })
end

function ImpactEventsSdk.read(ref: ComputedRef<number> | ComputedRef<boolean>): NumberRef | BoolRef
  local node = { op = "input", name = ref.slot }
  if (ref :: any).owner ~= nil then
    node.owner = (ref :: any).owner
  end
  if ref.kind == "number" then
    return numberRef(node)
  end
  return boolRef(node)
end

--- Public lifting helpers keep the raw-node adapters private to the SDK.
ImpactEventsSdk.fromRuntime = table.freeze({
  number = function(value: RuntimeValue): NumberRef
    return numberRef(value)
  end,
  bool = function(value: RuntimeValue): BoolRef
    return boolRef(value)
  end,
}) :: RuntimeExpressionRefs

local IMPACT_EFFECT_CAPABILITY: ImpactEffectCapability = table.freeze({ __impactEffectCapability = true })

local function lowerImpactEffect(effect: Effect): ImpactEffectWire
  return effect(IMPACT_EFFECT_CAPABILITY)
end

local function impactEffect(primitive: string, args: { [string]: any }?): Effect
  local wire = { primitive = primitive, target = "@impact.target", args = args } :: any
  return function(capability: ImpactEffectCapability): ImpactEffectWire
    if capability ~= IMPACT_EFFECT_CAPABILITY then
      error("impact effects can only be lowered by the SDK", 2)
    end
    return wire
  end
end

local function sourceImpactEffect(primitive: string, args: { [string]: any }?): Effect
  local wire = { primitive = primitive, target = "@impact.source", args = args } :: any
  return function(capability: ImpactEffectCapability): ImpactEffectWire
    if capability ~= IMPACT_EFFECT_CAPABILITY then
      error("impact effects can only be lowered by the SDK", 2)
    end
    return wire
  end
end

local IMPACT_TARGET: TargetHandle
local IMPACT_SOURCE: SourceHandle

local function sentimentImpactEffect(
  target: "@impact.target" | "@impact.source",
  toward: TargetHandle | SourceHandle,
  delta: NumberValue
): Effect
  local towardToken = if toward == IMPACT_TARGET then "@impact.target" elseif toward == IMPACT_SOURCE then "@impact.source" else "@invalid"
  local wire = {
    primitive = "adjustSentiment",
    target = target,
    args = { toward = towardToken, delta = numberNode(delta) },
  } :: any
  return function(capability: ImpactEffectCapability): ImpactEffectWire
    if capability ~= IMPACT_EFFECT_CAPABILITY then
      error("impact effects can only be lowered by the SDK", 2)
    end
    return wire
  end
end

-- The `@defense.` input a layer handle reads; a bad reference (a bare string,
-- a damage-type handle) names no layer, so its bind fails at install.
local function defenseInput(layer: any, capacity: boolean): string
  local name = "@invalid"
  if type(layer) == "table" and rawget(layer, "kind") == "defenseLayer" and type(rawget(layer, "name")) == "string" then
    name = rawget(layer, "name")
  end
  return "@defense." .. name .. (if capacity then ".capacity" else "")
end

IMPACT_TARGET = table.freeze({
  healthBefore = numberRef({ op = "input", name = "@impact.healthBefore" }),
  healthAfter = numberRef({ op = "input", name = "@impact.healthAfter" }),
  maxHealth = numberRef({ op = "input", name = "@impact.maxHealth" }),
  despawn = function(_self: TargetHandle, opts: { afterMs: number? }?): Effect
    return impactEffect("despawn", if opts == nil or opts.afterMs == nil then {} else { afterMs = opts.afterMs })
  end,
  playAnim = function(_self: TargetHandle, clip: string): Effect
    return impactEffect("playAnim", { clip = clip })
  end,
  setHealth = function(_self: TargetHandle, value: NumberValue, opts: { afterMs: number? }?): Effect
    local args: {[string]: any} = { value = numberNode(value) }
    if opts ~= nil and opts.afterMs ~= nil then
      args.afterMs = opts.afterMs
    end
    return impactEffect("setHealth", args)
  end,
  adjustSentimentToward = function(_self: TargetHandle, toward: TargetHandle | SourceHandle, delta: NumberValue): Effect
    return sentimentImpactEffect("@impact.target", toward, delta)
  end,
  state = function(_self: TargetHandle, name: string): NumberRef
    return numberRef({ op = "input", name = "@state." .. name })
  end,
  defense = function(_self: TargetHandle, layer: DefenseLayer)
    return table.freeze({
      current = numberRef({ op = "input", name = defenseInput(layer, false) }),
      capacity = numberRef({ op = "input", name = defenseInput(layer, true) }),
    })
  end,
  setState = function(_self: TargetHandle, name: string, value: NumberValue): Effect
    return impactEffect("setState", { name = name, value = numberNode(value) })
  end,
})

IMPACT_SOURCE = table.freeze({
  __impactSourceBrand = true,
  -- Engine state refs' `byPlayer` recognizes the source by this wire spelling.
  __wire = "@impact.source",
  grantHealth = function(_self: SourceHandle, amount: NumberValue): Effect
    return sourceImpactEffect("grantHealth", { amount = numberNode(amount) })
  end,
  grantAmmo = function(_self: SourceHandle, ammoType: string, amount: NumberValue): Effect
    return sourceImpactEffect("grantAmmo", { type = ammoType, amount = numberNode(amount) })
  end,
  grantLayer = function(_self: SourceHandle, layer: DefenseLayer, amount: NumberValue?): Effect
    local args: { [string]: any } = { layer = layer }
    if amount ~= nil then
      args.amount = numberNode(amount)
    end
    return sourceImpactEffect("grantLayer", args)
  end,
  inLifeState = function(_self: SourceHandle, name: string): BoolRef
    return boolRef({ op = "input", owner = "@impact.source", name = "player.inLifeState:" .. name })
  end,
  setLifeState = function(_self: SourceHandle, name: string): Effect
    return sourceImpactEffect("setLifeState", { state = name })
  end,
  adjustSentimentToward = function(_self: SourceHandle, toward: TargetHandle | SourceHandle, delta: NumberValue): Effect
    return sentimentImpactEffect("@impact.source", toward, delta)
  end,
}) :: any

local function assistSide(wire: string): AssistSide
  return table.freeze({
    __assistSideBrand = true,
    -- `byPlayer` recognizes each side by this wire spelling.
    __wire = wire,
    inLifeState = function(_self: AssistSide, name: string): BoolRef
      return boolRef({ op = "input", owner = wire, name = "player.inLifeState:" .. name })
    end,
  }) :: any
end

--- The helper and target of an assist pair, for `assistable.radius` and
--- `assistable.holdMs` expressions.
ImpactEventsSdk.assist = table.freeze({
  helper = assistSide("@assist.helper"),
  target = assistSide("@assist.target"),
}) :: Assist

-- A bad reference (a bare string, a layer handle) names no registered type,
-- so its input fails the bind and the policy holding it drops at install.
local function impactTypeName(handle: any): string
  if type(handle) == "table" and rawget(handle, "kind") == "damageType" and type(rawget(handle, "name")) == "string" then
    return rawget(handle, "name")
  end
  return "@invalid"
end

local IMPACT: Impact = table.freeze({
  target = IMPACT_TARGET,
  source = IMPACT_SOURCE,
  amount = numberRef({ op = "input", name = "@impact.amount" }),
  raw = numberRef({ op = "input", name = "@impact.raw" }),
  absorbed = numberRef({ op = "input", name = "@impact.absorbed" }),
  broke = boolRef({ op = "input", name = "@impact.broke" }),
  type = function(damageType: DamageType): BoolRef
    return boolRef({ op = "input", name = "@impact.type." .. impactTypeName(damageType) })
  end,
})

--- Spawn a passive presentation at the current impact target, shared with every
--- participating peer. The runtime freezes `value` and the anchor before despawn;
--- source identity does not restrict the audience.
function ImpactEventsSdk.present(template: { id: string }, value: NumberValue): Effect
  if type(template) ~= "table" or type(template.id) ~= "string" or #template.id == 0 then
    error("present: template must come from definePresentationTemplate", 2)
  end
  return impactEffect("present", { template = template.id, value = numberNode(value) })
end

local lowerImpactPolicy: (policy: {EffectOrGroup}) -> {EffectOrGroup}

local function impactEvent(id: string, filter: { tag: string? }, policy: {EffectOrGroup}, levels: {string}?, isOverride: boolean): ImpactEvent
  local handle: any = { kind = "impact", id = id, isOverride = isOverride, filter = filter, policy = policy }
  if levels ~= nil then
    handle.levels = levels
  end
  handle.override = function(_self: ImpactEvent, overrideFilter: ImpactEventOverrideFilter, build: (Impact) -> {EffectOrGroup}): ImpactEvent
    if type(overrideFilter.tag) ~= "string" then
      error("impact-event override filter requires `tag`", 2)
    end
    return impactEvent(id, table.freeze({ tag = overrideFilter.tag }), lowerImpactPolicy(build(IMPACT)), overrideFilter.levels, true)
  end
  return table.freeze(handle) :: any
end

local function denseImpactArrayLength(values: any, context: string): number
  local count = 0
  local maxIndex = 0
  for key in pairs(values) do
    if type(key) ~= "number" or key < 1 or key % 1 ~= 0 then
      error(context .. " must be a dense array; holes are not allowed", 3)
    end
    count += 1
    maxIndex = math.max(maxIndex, key)
  end
  if count ~= maxIndex then
    error(context .. " must be a dense array; holes are not allowed", 3)
  end
  return maxIndex
end

lowerImpactPolicy = function(policy: {EffectOrGroup}): {EffectOrGroup}
  local lowered: {EffectOrGroup} = descriptorArray({})
  local policyLength = denseImpactArrayLength(policy, "impact policy")
  for i = 1, policyLength do
    local entry = policy[i]
    if type(entry) == "function" then
      lowered[i] = lowerImpactEffect(entry :: Effect) :: any
    elseif type(entry) == "table" and entry["do"] ~= nil then
      local authoredEffects = entry["do"]
      local effectCount = denseImpactArrayLength(authoredEffects, "impact policy group `do`")
      local effects = descriptorArray({})
      for effectIndex = 1, effectCount do
        effects[effectIndex] = lowerImpactEffect(authoredEffects[effectIndex])
      end
      local group: any = { ["do"] = effects }
      if entry.when ~= nil then
        group.when = boolNode(entry.when)
      end
      lowered[i] = group
    else
      lowered[i] = entry
    end
  end
  return lowered
end

function ImpactEventsSdk.set(ref: Ref<number>, value: NumberValue): Effect
  local wire = {
    primitive = "slot.set",
    args = {
      slot = ref.slot,
      value = numberNode(value),
    },
  } :: any
  if (ref :: any).owner ~= nil then
    wire.target = (ref :: any).owner
  end
  return function(capability: ImpactEffectCapability): ImpactEffectWire
    if capability ~= IMPACT_EFFECT_CAPABILITY then
      error("impact effects can only be lowered by the SDK", 2)
    end
    return wire
  end
end

--- Build a read-modify-write effect. `build` receives `cur` during impact plan
--- phase, before effects apply. Owner-addressed reads resolve the live per-seat
--- map; `cur` is not a frozen snapshot.
function ImpactEventsSdk.update(ref: Ref<number>, build: (cur: NumberRef) -> NumberValue): Effect
  return ImpactEventsSdk.set(ref, build(ImpactEventsSdk.read(ref) :: NumberRef))
end

--- Build a deferred impact-effect group guarded by a Bool expression.
function ImpactEventsSdk.when(cond: BoolRef, effects: {Effect}): GatedEffect
  return { when = cond, ["do"] = effects }
end

local IMPACT_EVENT_ID_DIAGNOSTIC = "impact-event `id` must be a single ASCII segment using only [A-Za-z0-9_.-], at most 64 bytes; the engine prefixes the mod id"
local BINDING_NAME_SUGAR_DIAGNOSTIC = "defineStore/defineImpactEvent/defineDefenseLayer/defineDamageType/defineStatus without an explicit name is binding-name sugar and must be used in a direct top-level binding declaration"

local function validateImpactEventId(id: string)
  local valid = #id > 0 and #id <= 64
  for i = 1, #id do
    local byte = string.byte(id, i)
    local allowed = (byte >= 48 and byte <= 57)
      or (byte >= 65 and byte <= 90)
      or (byte >= 97 and byte <= 122)
      or byte == 95 or byte == 46 or byte == 45
    valid = valid and allowed
  end
  if not valid then
    error(IMPACT_EVENT_ID_DIAGNOSTIC, 3)
  end
end

--- Define a pure impact-policy descriptor. The engine prefixes its single-segment
--- authored id with the mod id. Luau requires the explicit-id form.
function ImpactEventsSdk.defineImpactEvent(...: any): ImpactEvent
  if select("#", ...) == 2 then
    error(BINDING_NAME_SUGAR_DIAGNOSTIC, 2)
  end
  local id, filter, build = ...
  validateImpactEventId(id)
  local eventFilter = table.freeze({ tag = filter.tag })
  local policy = lowerImpactPolicy(build(IMPACT))
  return impactEvent(id, eventFilter, policy, filter.levels, false)
end

return {
  builders = ImpactEventsSdk,
  BINDING_NAME_SUGAR_DIAGNOSTIC = BINDING_NAME_SUGAR_DIAGNOSTIC,
}
