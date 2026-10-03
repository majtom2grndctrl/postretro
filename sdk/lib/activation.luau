--!strict
-- Pure weapon-action builders. Rust validates structure, input scope, and domains.
local expressionRefs = __postretroExpressionRefs
local numberNode = expressionRefs.numberNode
local numberRef = expressionRefs.numberRef

--- Independent shot multipliers. Omitted axes use 1; expressions may read only `activation.charge`.
export type ActivationShotScale = {
  --- Direct or splash damage multiplier, applied once. Finite range: [0, 64].
  damage: NumberValue?,
  --- Hitscan distance or projectile travel cap multiplier. Finite range: (0, 64].
  range: NumberValue?,
  --- Projectile travel-speed multiplier. Finite range: (0, 64].
  projectileSpeed: NumberValue?,
  --- Projectile swept collision-radius multiplier. Finite range: (0, 64].
  projectileRadius: NumberValue?,
  --- Projectile visual-size multiplier; collision and splash radius are independent. Finite range: (0, 64].
  projectileSize: NumberValue?,
  --- Direct and splash knockback-speed multiplier. Finite range: [0, 64].
  knockbackSpeed: NumberValue?,
  --- Per-shot ammo, heat, or cell-cost multiplier. Finite range: (0, 64]; positive ammo costs round up.
  resourceCost: NumberValue?,
}

--- Shot options. Multipliers accept finite literals, fluent refs, or numeric `runtime.*` IR.
export type ActivationShotOptions = {
  --- Independent multipliers, defaulting to 1. Only the read-only `charge` input is allowed.
  scale: ActivationShotScale?,
}

--- Closed weapon-action data builders. Programs allow at most 64 steps and 16 shots.
export type Activation = {
  --- Normalized start-to-release charge in [0, 1]; uncharged actions read 1. No state-store reads or writes. Use `activation.charge:times(9):plus(1)`.
  charge: NumberRef,
  --- Build an ordinary shot. First and last steps must be shots, with a positive wait between shots.
  shot: (options: ActivationShotOptions?) -> ActivationStepDescriptor,
  --- Build a wait in milliseconds: finite (0, 60000], rounded up to whole ticks. Total quantized waits are at most 60 seconds.
  wait: (durationMs: number) -> ActivationStepDescriptor,
}

local function shot(options: ActivationShotOptions?): ActivationStepDescriptor
  if options == nil or options.scale == nil then
    return table.freeze({ kind = "shot" }) :: any
  end
  local scale: {[string]: any} = {}
  for key, value in pairs(options.scale :: any) do
    scale[key] = if type(value) == "number" then value else numberNode(value)
  end
  return table.freeze({ kind = "shot", scale = table.freeze(scale) }) :: any
end

return table.freeze({
  charge = numberRef({ op = "input", name = "charge" }),
  shot = shot,
  wait = function(durationMs: number): ActivationStepDescriptor
    return table.freeze({ kind = "wait", durationMs = durationMs }) :: any
  end,
}) :: Activation
