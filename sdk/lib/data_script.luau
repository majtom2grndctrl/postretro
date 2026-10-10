--!strict
-- Data-script vocabulary: pure descriptor builders for `ModManifest` and
-- `setupLevel`. `defineReaction` and `defineEntity` construct
-- typed plain tables that the engine deserializes from the script's
-- return value. They never call back into Rust -- the FFI boundary is the
-- `return` statement of the script entry point.
-- See: context/lib/scripting.md §2 (Data context lifecycle)
--
-- ---------------------------------------------------------------------
-- Canonical author example -- entity types registered via the mod manifest,
-- per-level reactions via `setupLevel`.
--
-- ```lua
-- return defineMod({
--   name = "MyMod",
--   id = "example.my-mod",
--   version = "1.0.0",
--   -- Entity types are returned from the mod manifest and survive level
--   -- unload. Use `defineEntity` for a typed construction site at the FFI
--   -- boundary.
--   entities = {
--     defineEntity({
--       classname = "light_dynamic",
--       components = { light = { color = { x = 1, y = 0.8, z = 0.6 }, intensity = 1.0, range = 6.0, is_dynamic = true } },
--     }),
--   },
-- })
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

-- Empty Luau tables have no sequence/map distinction. Runtime states provide
-- mlua's private array metatable while this SDK module loads; capturing it here
-- preserves only builder-owned descriptor arrays across serde. Standalone SDK
-- evaluation used by focused tests falls back to ordinary tables.
local DESCRIPTOR_ARRAY_METATABLE = rawget(_G, "__postretroArrayMetatable")

local function descriptorArray(values: any): any
  if DESCRIPTOR_ARRAY_METATABLE == nil then
    return values
  end
  return setmetatable(values, DESCRIPTOR_ARRAY_METATABLE)
end

--- Progress-subscription reaction body: fires `fire` when entities tagged
--- `tag` cross the kill ratio `at` (0.0–1.0).
export type ProgressReactionDescriptor = {
  progress: { tag: string, at: number, fire: string },
}

--- Primitive reaction body: invokes the named Rust primitive. A group command
--- (`npcs(...)`, `players()`) carries `kind` and an optional `tag` filter; a
--- subject-token command (`on.activators`, `on.trigger`) carries `target =
--- "@activators"` or `target = "@trigger"`. A raw descriptor with only a non-empty `tag` resolves over every entity
--- carrying it (fog, emitter and animation primitives have no typed builder).
--- True system reactions carry neither and enqueue typed engine commands such
--- as `playSound`, `rumble`, `flashScreen`, and the UI-stack reactions. `args`
--- carries the primitive's typed payload (e.g. `{ rate = 0 }` for
--- `setEmitterRate`).
export type PrimitiveReactionDescriptor = {
  primitive: string,
  kind: GroupKind?,
  tag: string?,
  target: SubjectTokenTarget?,
  args: { [string]: any }?,
  onComplete: string?,
}

--- Kinds a group command addresses.
export type GroupKind = "npc" | "player"

--- One group command: a primitive descriptor carrying `kind` (and an optional
--- `tag` filter) instead of a target. Legal both as a reaction body and
--- directly as a sequence entry; the group resolves when the command takes
--- effect.
export type GroupCommand = {
  primitive: string,
  kind: GroupKind,
  tag: string?,
  args: { [string]: any },
}

--- The subject a token command addresses: this fire's activators or the
--- volume that fired.
export type SubjectTokenTarget = "@activators" | "@trigger"

--- One subject-token command (`on.activators`, `on.trigger`): a primitive
--- descriptor whose `target` names the fire's subject. Legal both as a reaction
--- body and directly as a sequence entry, but only before any `wait`.
export type SubjectTokenCommand = {
  primitive: string,
  target: SubjectTokenTarget,
  args: { [string]: any },
}

--- Selects the NPCs a group command reaches. Omit `tag` for every NPC.
export type NpcGroupFilter = { tag: string? }

--- Typed, additive partial for NPC state updates.
export type NpcStateUpdateArgs = { aggro: boolean? }

--- Brain-driven characters that are not players. Opaque: no members, no
--- length; indexing anything but a verb raises.
export type NpcGroup = {
  update: (self: NpcGroup, fields: NpcStateUpdateArgs) -> GroupCommand,
  damage: (self: NpcGroup, amount: number) -> GroupCommand,
}

--- Every seat-bound player pawn. Opaque: no members, no length; indexing
--- anything but a verb raises.
export type PlayerGroup = {
  damage: (self: PlayerGroup, amount: number) -> GroupCommand,
  grantHealth: (self: PlayerGroup, amount: number) -> GroupCommand,
  grantAmmo: (self: PlayerGroup, type: string, amount: number) -> GroupCommand,
  addSlot: (self: PlayerGroup, slot: StateRef<number>, delta: number) -> GroupCommand,
}

--- Tween shape used by the billboard-emitter reaction primitive
--- `setSpinRate`. Mirrors the Rust `SpinAnimation` storage struct.
export type SpinAnimation = {
  duration: number,
  rate_curve: {number},
}

--- One step in a `sequence` reaction body. Sequence steps target a single
--- `EntityId`; tag-targeted primitives (`setEmitterRate`, billboard-emitter
--- `setSpinRate`) belong on the `reactions:` -> `Primitive` path, not on
--- `sequence`.
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

export type SetFogGlowStep = {
  id: number,
  primitive: "setFogGlow",
  args: { glow: number },
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
  args: { density: number?, glow: number?, edgeSoftness: number?, falloff: number? },
}

export type MoverStartStep = {
  id: number,
  primitive: "moverStart",
  args: {},
}

export type MoverStopStep = {
  id: number,
  primitive: "moverStop",
  args: {},
}

export type MoverReverseStep = {
  id: number,
  primitive: "moverReverse",
  args: {},
}

export type MoverGoToPathNodeStep = {
  id: number,
  primitive: "moverGoToPathNode",
  args: { node: string },
}

export type MoverSetSpinRateStep = {
  id: number,
  primitive: "moverSetSpinRate",
  args: { rate: number },
}

export type MoverSetBlockPolicyStep = {
  id: number,
  primitive: "moverSetBlockPolicy",
  args: { policy: "displace" | "reverse" | "stop" | "crush" },
}

export type ArmTriggerStep = {
  id: number,
  primitive: "armTrigger",
  args: {},
}

export type DisarmTriggerStep = {
  id: number,
  primitive: "disarmTrigger",
  args: {},
}

--- Member step: spawn one batch from one spawner (`s:fire()`).
export type SpawnFromSpawnerStep = {
  id: number,
  primitive: "spawnFromSpawner",
}

export type WaitArgs = { durationMs: number, interruptible: boolean }
export type FireArgs = { event: string }

--- Control step: enrolls the rest of the sequence body with the host
--- scheduler and stops; the tail resumes after `durationMs` (rounded up to
--- whole authoritative ticks).
export type WaitStep = {
  id: "@wait",
  primitive: "wait",
  args: WaitArgs,
}

--- Control step: dispatches a named reaction from inside a sequence body.
export type FireStep = {
  id: "@fire",
  primitive: "fire",
  args: FireArgs,
}

-- `SetFogAnimationStep` is declared in `postretro.d.luau` (the SDK lib
-- block generated from the primitive registry) and is not redeclared here.
--- Ordered member, group, subject-token and control steps. Steps begin in
--- array order; `fire` queues a named dispatch, while `wait` stops the current
--- drain and resumes the remaining tail after its delay.
export type SequenceStep = SetLightAnimationStep | SetFogDensityStep | SetFogGlowStep | SetFogEdgeSoftnessStep | SetFogFalloffStep | SetFogParamsStep | SetFogAnimationStep | MoverStartStep | MoverStopStep | MoverReverseStep | MoverGoToPathNodeStep | MoverSetSpinRateStep | MoverSetBlockPolicyStep | ArmTriggerStep | DisarmTriggerStep | SpawnFromSpawnerStep | GroupCommand | SubjectTokenCommand | WaitStep | FireStep

--- Descriptor produced by `defineReaction`. The `name` field is merged
--- into the descriptor at the top level so the Rust deserializer can read
--- both the event name and the descriptor body from a single flat table.
export type NamedReactionDescriptor = {
  name: string,
  levels: {string}?,
  -- progress shape
  progress: { tag: string, at: number, fire: string }?,
  -- primitive shape
  primitive: string?,
  kind: GroupKind?,
  tag: string?,
  target: SubjectTokenTarget?,
  args: { [string]: any }?,
  onComplete: string?,
  -- sequence shape: member, group, subject-token and wait/fire control steps
  sequence: {SequenceStep}?,
}

export type CrossingParams = { rising: RuntimeRead }
export type TickParams = { dt: RuntimeRead }
--- The pawns that caused the current trigger edge. Legal only before any `wait`.
export type ActivatorsTarget = {
  damage: (self: ActivatorsTarget, amount: number) -> SubjectTokenCommand,
  grantHealth: (self: ActivatorsTarget, amount: number) -> SubjectTokenCommand,
  grantAmmo: (self: ActivatorsTarget, type: string, amount: number) -> SubjectTokenCommand,
  addSlot: (self: ActivatorsTarget, slot: StateRef<number>, delta: number) -> SubjectTokenCommand,
}
--- The trigger volume that fired the current edge. Legal only before any `wait`.
export type TriggerTarget = {
  arm: (self: TriggerTarget) -> SubjectTokenCommand,
  disarm: (self: TriggerTarget) -> SubjectTokenCommand,
}
export type TriggerEventParams = {
  activators: ActivatorsTarget,
  trigger: TriggerTarget,
  occupancy: RuntimeRead,
}
--- Opaque anchor for where the current named gameplay event happened. Legal
--- only as `playSound`'s `at`.
export type EmitterTarget = { __emitterTarget: true }
--- Dispatch values published by a named gameplay event: weapon, reload,
--- impact, enemy, movement and mover events.
export type EmitterParams = { emitter: EmitterTarget }
export type Reaction<S> = NamedReactionDescriptor

export type NumberValue = number | NumberRef | RuntimeValue
export type BoolValue = boolean | BoolRef | RuntimeValue

export type NumberRef = {
  plus: (self: NumberRef, n: NumberValue) -> NumberRef,
  minus: (self: NumberRef, n: NumberValue) -> NumberRef,
  times: (self: NumberRef, n: NumberValue) -> NumberRef,
  dividedBy: (self: NumberRef, n: NumberValue) -> NumberRef,
  clamp: (self: NumberRef, lo: NumberValue, hi: NumberValue) -> NumberRef,
  lerp: (self: NumberRef, to: NumberValue, t: NumberValue) -> NumberRef,
  lt: (self: NumberRef, n: NumberValue) -> BoolRef,
  le: (self: NumberRef, n: NumberValue) -> BoolRef,
  gt: (self: NumberRef, n: NumberValue) -> BoolRef,
  ge: (self: NumberRef, n: NumberValue) -> BoolRef,
  eq: (self: NumberRef, n: NumberValue) -> BoolRef,
  ne: (self: NumberRef, n: NumberValue) -> BoolRef,
}

export type BoolRef = {
  --- `and`, `or`, and `not` are Luau keywords. Call this as
  --- `ref["and"](ref, other)`, not `ref:and(other)`.
  ["and"]: (self: BoolRef, other: BoolValue) -> BoolRef,
  ["or"]: (self: BoolRef, other: BoolValue) -> BoolRef,
  ["not"]: (self: BoolRef) -> BoolRef,
  select: (self: BoolRef, whenTrue: NumberValue, whenFalse: NumberValue) -> NumberRef,
}

--- Lift raw `runtime.*` output into the fluent impact-expression algebra.
--- The `number` and `bool` helpers select the expected result kind; Rust
--- validates the resulting IR when it binds the descriptor.
export type RuntimeExpressionRefs = {
  number: (value: RuntimeValue) -> NumberRef,
  bool: (value: RuntimeValue) -> BoolRef,
}

type ImpactEffectWire =
  { primitive: "despawn", target: "@impact.target", args: { afterMs: number? } }
  | { primitive: "playAnim", target: "@impact.target", args: { clip: string } }
  | { primitive: "present", target: "@impact.target", args: { template: string, value: any } }
  | { primitive: "setHealth", target: "@impact.target", args: { value: any, afterMs: number? } }
  | { primitive: "setState", target: "@impact.target", args: { name: string, value: any } }
  | { primitive: "grantHealth", target: "@impact.source", args: { amount: any } }
  | { primitive: "grantAmmo", target: "@impact.source", args: { type: string, amount: any } }
  | { primitive: "adjustSentiment", target: "@impact.target" | "@impact.source", args: { toward: "@impact.target" | "@impact.source", delta: any } }
  | { primitive: "slot.set", args: { slot: string, value: any } }
  | { primitive: "slot.set", target: "@impact.source", args: { slot: string, value: any } }
type ImpactEffectCapability = { __impactEffectCapability: true }
-- Constructors retain the only lowering capability. A hand-authored table is
-- not an Effect, and every value that type-checks lowers to one closed wire arm.
export type Effect = (ImpactEffectCapability) -> ImpactEffectWire
export type GatedEffect = { when: BoolRef?, ["do"]: {Effect} }
export type EffectOrGroup = Effect | GatedEffect
export type ImpactEventFilter = { tag: string?, levels: {string}? }
export type ImpactEventOverrideFilter = { tag: string, levels: {string}? }

export type TargetHandle = {
  healthBefore: NumberRef,
  healthAfter: NumberRef,
  maxHealth: NumberRef,
  despawn: (self: TargetHandle, opts: { afterMs: number? }?) -> Effect,
  playAnim: (self: TargetHandle, clip: string) -> Effect,
  --- Clamp to the health range. Only a positive stored result recovers and
  --- re-arms; zero stays down. Literals must be finite, and non-finite IR
  --- arithmetic resolves to zero.
  setHealth: (self: TargetHandle, value: NumberValue, opts: { afterMs: number? }?) -> Effect,
  --- Adjust this faction's sentiment toward another impact recipient. Negative
  --- values degrade the relationship; positive values strengthen its bond.
  adjustSentimentToward: (self: TargetHandle, toward: TargetHandle | SourceHandle, delta: NumberValue) -> Effect,
  state: (self: TargetHandle, name: string) -> NumberRef,
  setState: (self: TargetHandle, name: string, value: NumberValue) -> Effect,
}

export type SourceHandle = {
  __impactSourceBrand: true,
  --- Add health to the impact damager. A fire with no damager skips this
  --- effect; app-drain impacts run no policy in v1. Amount expressions read
  --- impact-target facts/state only: v1 has no source-scoped fact vocabulary.
  grantHealth: (self: SourceHandle, amount: NumberValue) -> Effect,
  --- Add an ammo-pool balance to the impact damager. A fire with no damager
  --- skips this effect; app-drain impacts run no policy in v1. Amount
  --- expressions remain impact-target scoped; v1 has no source facts.
  grantAmmo: (self: SourceHandle, type: string, amount: NumberValue) -> Effect,
  --- Adjust this faction's sentiment toward another impact recipient. Negative
  --- values degrade the relationship; positive values strengthen its bond.
  adjustSentimentToward: (self: SourceHandle, toward: TargetHandle | SourceHandle, delta: NumberValue) -> Effect,
}

export type Impact = {
  target: TargetHandle,
  source: SourceHandle,
  amount: NumberRef,
}

export type ImpactEvent = {
  kind: "impact",
  isOverride: boolean,
  levels: {string}?,
  override: (self: ImpactEvent, filter: ImpactEventOverrideFilter, build: (Impact) -> {EffectOrGroup}) -> ImpactEvent,
}

--- Bundle returned from `setupLevel`. The engine deserializes
--- this shape in one pass at level load and drops the data-script VM
--- context after the return.
--- `entities` is absent: entity-type registrations belong in
--- `ModManifest.entities`, not here.
export type LevelManifest = {
  reactions: {NamedReactionDescriptor},
  events: {ImpactEvent}?,
  -- State-crossing watchers (HUD dynamics). `CrossingDescriptor` is declared
  -- in postretro.d.luau (the generated SDK-lib type block). See
  -- `onStateCrossing`.
  crossings: {CrossingDescriptor}?,
  -- Level trigger events, keyed by volume: build each with a trigger member's `on`.
  triggerEvents: {VolumeTriggerEventDescriptor}?,
  triggerPools: {TriggerPoolDescriptor}?,
  -- Per-level UI trees (name + `AnchoredTree` + optional `alwaysOn` / `hideBelow`). Optional; same shape
  -- as `ModManifest.uiTrees` but level-scoped (cleared on unload). Malformed
  -- entries are logged and skipped.
  uiTrees: {ModUiTree}?,
}

--- One slot inside a `defineStore` schema. Every slot needs `default`.
--- `type = "number"` accepts a finite numeric default plus optional inclusive
--- `range = {min, max}`; `"boolean"` and `"string"` require matching defaults;
--- `"enum"` requires non-empty `values` and a default in that list; `"array"` is
--- a finite-number array. `persist` saves global slots on clean exit. On
--- connected clients, `perOwner = true, persist = true` saves the local
--- owner's value every ~60 seconds and on clean exit; it travels via that
--- player's join seed. `readonly` blocks script writes. A mod-owned persisted writable or replicated slot requires
--- a minted `<mod-root>/identity.json` entry; run
--- `postretro-tool mint-identity <mod>`. Keep its durable key when
--- renaming the store or slot.
--- `perOwner = true` creates a separate host-side value for each player seat.
--- It permits omitted `network` (host-local) or `"ownerPrivate"`; `"shared"`
--- is reserved for global slots.
export type StoreSlotSchema = (
  { type: "number", readonly: boolean?, network: "shared"?, perOwner: false?, accumulate: nil? }
  | { type: "number", readonly: boolean?, network: "ownerPrivate"?, perOwner: true, persist: boolean?, accumulate: nil? }
  | { type: "number", readonly: false?, network: "shared"?, perOwner: false?, accumulate: (TickParams) -> RuntimeValue }
  | { type: "boolean" | "string" | "enum" | "array", readonly: boolean?, network: "shared"?, perOwner: false?, accumulate: nil? }
  | { type: "boolean" | "string" | "enum" | "array", readonly: boolean?, network: "ownerPrivate"?, perOwner: true, persist: boolean?, accumulate: nil? }
) & { [string]: any }
export type StoreDeclaration = {
  namespace: string,
  schema: { [string]: StoreSlotSchema },
}
export type StateRefKind = "number" | "boolean" | "string" | "enum" | "array"
export type ComputedRef<T> = { slot: string, kind: StateRefKind, __stateRefValueBrand: T? }
export type Ref<T> = ComputedRef<T> & { __writableStateRefBrand: T }
export type OwnerAddressedComputedRef<T> = ComputedRef<T> & { owner: "@impact.source" }
export type OwnerAddressedRef<T> = Ref<T> & { owner: "@impact.source" }
type StoreComputedRef<T> = ComputedRef<T> & {
  byPlayer: (self: StoreComputedRef<T>, owner: SourceHandle) -> OwnerAddressedComputedRef<T>,
}
type StoreRef<T> = Ref<T> & {
  byPlayer: (self: StoreRef<T>, owner: SourceHandle) -> OwnerAddressedRef<T>,
}
export type StateRef<T> = StoreComputedRef<T> | StoreRef<T>
export type StoreDefinition = {
  [string]: StateRef<any>,
}

local DataScriptSdk = {}
local storeDeclarations = setmetatable({}, { __mode = "k" })

-- Part chunks (`data_script/*.luau`) evaluate before this module and publish
-- through the temporary `__postretroDataScriptParts` bridge; the host clears it
-- once this module returns. `defineStore` traces accumulators with the same
-- shared dispatch params as `defineReaction`.
local dataScriptParts = __postretroDataScriptParts
local DISPATCH_PARAMS = dataScriptParts.reactions.DISPATCH_PARAMS

local expressionRefs = __postretroExpressionRefs
local numberNode = expressionRefs.numberNode
local boolNode = expressionRefs.boolNode
local numberRef = expressionRefs.numberRef
local boolRef = expressionRefs.boolRef

function DataScriptSdk.read(ref: ComputedRef<number> | ComputedRef<boolean>): NumberRef | BoolRef
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
DataScriptSdk.fromRuntime = table.freeze({
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
  setState = function(_self: TargetHandle, name: string, value: NumberValue): Effect
    return impactEffect("setState", { name = name, value = numberNode(value) })
  end,
})

IMPACT_SOURCE = table.freeze({
  __impactSourceBrand = true,
  grantHealth = function(_self: SourceHandle, amount: NumberValue): Effect
    return sourceImpactEffect("grantHealth", { amount = numberNode(amount) })
  end,
  grantAmmo = function(_self: SourceHandle, ammoType: string, amount: NumberValue): Effect
    return sourceImpactEffect("grantAmmo", { type = ammoType, amount = numberNode(amount) })
  end,
  adjustSentimentToward = function(_self: SourceHandle, toward: TargetHandle | SourceHandle, delta: NumberValue): Effect
    return sentimentImpactEffect("@impact.source", toward, delta)
  end,
}) :: any

local IMPACT: Impact = table.freeze({
  target = IMPACT_TARGET,
  source = IMPACT_SOURCE,
  amount = numberRef({ op = "input", name = "@impact.amount" }),
})

--- Spawn a passive presentation at the current impact target. The impact
--- runtime captures `value` before any following despawn and stamps source.
function DataScriptSdk.present(template: { id: string }, value: NumberValue): Effect
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

function DataScriptSdk.set(ref: Ref<number>, value: NumberValue): Effect
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
function DataScriptSdk.update(ref: Ref<number>, build: (cur: NumberRef) -> NumberValue): Effect
  return DataScriptSdk.set(ref, build(DataScriptSdk.read(ref) :: NumberRef))
end

--- Build a deferred impact-effect group guarded by a Bool expression.
function DataScriptSdk.when(cond: BoolRef, effects: {Effect}): GatedEffect
  return { when = cond, ["do"] = effects }
end

local IMPACT_EVENT_ID_DIAGNOSTIC = "impact-event `id` must be a single ASCII segment using only [A-Za-z0-9_.-], at most 64 bytes; the engine prefixes the mod id"
local BINDING_NAME_SUGAR_DIAGNOSTIC = "defineStore/defineImpactEvent without an explicit name is binding-name sugar and must be used in a direct top-level binding declaration"

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
function DataScriptSdk.defineImpactEvent(...: any): ImpactEvent
  if select("#", ...) == 2 then
    error(BINDING_NAME_SUGAR_DIAGNOSTIC, 2)
  end
  local id, filter, build = ...
  validateImpactEventId(id)
  local eventFilter = table.freeze({ tag = filter.tag })
  local policy = lowerImpactPolicy(build(IMPACT))
  return impactEvent(id, eventFilter, policy, filter.levels, false)
end

--- Identity builder for entity type descriptors returned from
--- `ModManifest.entities`. `descriptor` is the full archetype object: optional
--- `canonicalName`, optional `components.inventory.loadout`, and optional component presets.
--- Pure: no engine side effects.
local function hasNumericKey(value: any): boolean
  for key in pairs(value) do
    if type(key) == "number" then
      return true
    end
  end
  return false
end

local function lowerLoadoutReferences(descriptor: EntityTypeDescriptor)
  local components = descriptor.components
  if components == nil or components.inventory == nil or components.inventory.loadout == nil then
    return
  end

  local loadout = components.inventory.loadout
  for index, entry in ipairs(loadout) do
    local entryName = "components.inventory.loadout[" .. tostring(index - 1) .. "]"
    if type(entry) ~= "table" or hasNumericKey(entry) then
      error(entryName .. " must reference an entity descriptor", 3)
    end
    if type(entry.components) ~= "table"
      or type(entry.components.weapon) ~= "table"
      or hasNumericKey(entry.components.weapon)
    then
      error(entryName .. " must reference a descriptor with a weapon block", 3)
    end
    if type(entry.canonicalName) ~= "string" or entry.canonicalName == "" then
      error(entryName .. " must reference a descriptor with a canonical name", 3)
    end
    (loadout :: any)[index] = entry.canonicalName
  end
end

function DataScriptSdk.defineEntity<T>(descriptor: T & EntityTypeDescriptor): T
  lowerLoadoutReferences(descriptor)
  return descriptor
end

--- Identity builder for the mod manifest consumed from the chunk return.
--- `config.name`, `config.id`, and `config.version` are required. The id gates
--- multiplayer admission; the version is display-only and never compared. The
--- first committed id and version remain active across staged reloads. Optional
--- arrays include `entities`, `factions`, `sentiment`, `maps`, `uiTrees`, `presentationTemplates`,
--- `reactions`, `events`, `crossings`, `triggerEvents`, `triggerPools`, and
--- `stores`; `presentationOverlays` accepts one descriptor. Pure: no engine side
--- effects until the manifest is returned and validated. `factionSentimentDecay`
--- is an optional non-negative global return-to-baseline rate; it defaults to
--- zero (hold), and an individual sentiment row may override it with `decay`.
function DataScriptSdk.defineMod<T>(config: T & { stores: any? }): T
  if config.stores == nil then
    return config
  end

  local manifest = table.clone(config)
  local stores = {}
  for key, entry in pairs(config.stores) do
    local declaration = if type(entry) == "table" then storeDeclarations[entry] else nil
    stores[key] = if declaration ~= nil then declaration else entry
  end
  manifest.stores = stores
  return manifest
end

--- Pure builder for a stable named faction declaration. Include the result in
--- `defineMod({ factions = { ... } })`; archetypes refer to the name through
--- `components.faction`. Names beginning with `@postretro.` are engine-reserved.
function DataScriptSdk.defineFaction(name: string): FactionDescriptor
  return { name = name }
end

--- Build one directed faction relationship for `sentiment` in the returned
--- mod manifest. Negative sentiment is hostile, zero neutral, and positive
--- allied; the engine validates both faction names at manifest drain. An
--- optional non-negative `decay` rate overrides `factionSentimentDecay`; zero
--- holds this relationship after a runtime change.
function DataScriptSdk.sentiment(
  fromFaction: string,
  toFaction: string,
  values: { sentiment: number, tolerance: number, decay: number? }
): FactionSentimentDescriptor
  return {
    fromFaction = fromFaction,
    toFaction = toFaction,
    sentiment = values.sentiment,
    tolerance = values.tolerance,
    decay = values.decay,
  }
end

--- Identity builder for a mod map catalog. `entries` are `ModMapEntry` tables
--- with required `id`, `path`, and `name`; optional `tags` default to empty and
--- drive filtering plus `levels` selectors. Pure: no engine side effects.
function DataScriptSdk.defineMapCatalog(entries: {ModMapEntry}): {ModMapEntry}
  return entries
end

--- Pure identity builder for reusable first-person weapon placement data. The
--- returned descriptor may be shared by weapon `placement` fields and
--- `defineMod({ defaultWeaponPlacement = ... })`; no engine registration occurs.
function DataScriptSdk.defineWeaponPlacement(desc: WeaponPlacementDescriptor): WeaponPlacementDescriptor
  return desc
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

local function storeRef(slot: string, kind: StateRefKind, perOwner: boolean): StateRef<any>
  local ref = { slot = slot, kind = kind }
  local methods = {
    byPlayer = function(_self: any, owner: SourceHandle): OwnerAddressedRef<any>
      if not perOwner then
        error("state slot `" .. slot .. "` is global and cannot be addressed with byPlayer", 2)
      end
      return table.freeze({
        slot = slot,
        kind = kind,
        owner = if owner == IMPACT_SOURCE then "@impact.source" else "@invalid",
      }) :: any
    end,
  }
  return table.freeze(setmetatable(ref, { __index = methods })) :: any
end

--- Pure state-store builder. `namespace` prefixes every returned state ref as
--- `namespace.slotName`; neither it nor a slot name may contain `:`. Luau
--- requires the explicit-namespace form.
--- Pass the returned handle through `defineMod({ stores = { store } })` to
--- resolve its declaration; unreturned handles disappear with the setup VM.
function DataScriptSdk.defineStore(...: any): StoreDefinition
  if select("#", ...) == 1 then
    error(BINDING_NAME_SUGAR_DIAGNOSTIC, 2)
  end
  local namespace, schema = ...
  if string.find(namespace, ":", 1, true) ~= nil then
    error("defineStore `namespace` must not contain `:`", 2)
  end
  local tracedSchema = {}
  for slotName, input in pairs(schema) do
    if string.find(slotName, ":", 1, true) ~= nil then
      error("defineStore slot name " .. string.format("%q", slotName) .. " must not contain `:`", 2)
    end
    if type(input) == "table" and type((input :: any).accumulate) == "function" then
      local tracedInput = {}
      for key, value in pairs(input :: any) do
        tracedInput[key] = value
      end
      tracedInput.accumulate = (input :: any).accumulate(DISPATCH_PARAMS)
      tracedSchema[slotName] = tracedInput
    else
      tracedSchema[slotName] = input
    end
  end
  local frozenSchema = cloneAndFreeze(tracedSchema)
  local store: { [string]: StateRef<any> } = {}
  for slotName in pairs(frozenSchema) do
    store[slotName] = storeRef(
      namespace .. "." .. slotName,
      (frozenSchema[slotName] :: any).type,
      (frozenSchema[slotName] :: any).perOwner == true
    )
  end
  local handle = table.freeze(store)
  storeDeclarations[handle] = table.freeze({
    namespace = namespace,
    schema = frozenSchema,
  })
  return handle
end

-- Merge the part chunks' public builders. Field names are disjoint from the
-- builders declared above; `data_script_export_inventory_matches_returned_sdk_fields`
-- pins the merged set.
for _, part in ipairs({ dataScriptParts.commands, dataScriptParts.reactions, dataScriptParts.triggerEvents }) do
  for name, builder in pairs(part.builders) do
    DataScriptSdk[name] = builder
  end
end

return DataScriptSdk
