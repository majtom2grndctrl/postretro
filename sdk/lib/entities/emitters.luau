--!strict
-- Reference vocabulary for authoring `BillboardEmitter` components in Luau.
--
-- Mirrors `sdk/lib/entities/emitters.ts`. Exports:
--   - `BillboardEmitter` — entity-shape type for named instances and
--     per-instance overrides. The engine handles
--     `classname "billboard_emitter"` natively (see plan-3 sub-plan 6).
--   - `SpinAnimation` — tween shape for the `setSpinRate` reaction
--     primitive.
--   - `ComponentDescriptor` — the shape every component constructor
--     returns (`{ kind, value }`).
--   - `emitter(props)` — pure component constructor with synchronous
--     validation and default-fill.
--   - `smokeEmitter`, `sparkEmitter`, `dustEmitter` — preset wrappers.
--
-- Naming intent: `BillboardEmitter` reserves `ParticleEmitter` for
-- future mesh-particle work; the rendering primitive is in the type
-- name so renames stay deliberate.
--
-- See: context/lib/scripting.md §11

-- ---------------------------------------------------------------------
-- Public types
-- ---------------------------------------------------------------------

--- Tween shape consumed by `setSpinRate`. Mirrors the Rust
--- `SpinAnimation` storage struct.
export type SpinAnimation = {
  duration: number,
  rate_curve: {number},
}

--- Shape of a `BillboardEmitter` value. Authors use this type for
--- named instances or per-instance overrides:
---
--- ```luau
--- local exhaustPort: BillboardEmitter = { rate = 50, buoyancy = 0.0 }
--- ```
export type BillboardEmitter = {
  rate: number?,
  burst: number?,
  spread: number?,
  lifetime: number?,
  velocity: {number}?,
  buoyancy: number?,
  drag: number?,
  size_over_lifetime: {number}?,
  opacity_over_lifetime: {number}?,
  color: {number}?,
  sprite: string?,
  spin_rate: number?,
}

--- Input shape for `emitter()`. Required: `lifetime`,
--- `velocity`, `sprite`. Other fields fall back to documented
--- defaults inside `emitter()`.
export type EmitterProps = {
  rate: number?,
  burst: number?,
  spread: number?,
  lifetime: number,
  velocity: {number},
  buoyancy: number?,
  drag: number?,
  size_over_lifetime: {number}?,
  opacity_over_lifetime: {number}?,
  color: {number}?,
  sprite: string,
  spin_rate: number?,
}

--- Flat `ComponentValue` shape produced by component constructors.
--- `kind` is the snake_case wire tag; sibling fields carry the component
--- payload directly (no `value` wrapper).
export type ComponentDescriptor = {
  kind: string,
  [string]: any,
}

local EmittersSdk = {}

-- ---------------------------------------------------------------------
-- Helpers
-- ---------------------------------------------------------------------

local function isFiniteNumber(v: any): boolean
  if type(v) ~= "number" then
    return false
  end
  -- NaN: v ~= v. +/-inf: compare to math.huge.
  if v ~= v or v == math.huge or v == -math.huge then
    return false
  end
  return true
end

local function validateVec3(v: any, field: string)
  if type(v) ~= "table" or #v ~= 3 then
    error(`emitter: \`{field}\` must be a 3-element [number, number, number]`)
  end
  for i = 1, 3 do
    if not isFiniteNumber(v[i]) then
      -- 0-based index for parity with the TypeScript error messages.
      error(`emitter: \`{field}\` element {i - 1} is not a finite number`)
    end
  end
end

local function validateCurve(curve: any, field: string)
  if type(curve) ~= "table" or #curve == 0 then
    error(`emitter: \`{field}\` must be a nonempty number array`)
  end
  for i = 1, #curve do
    if not isFiniteNumber(curve[i]) then
      error(`emitter: \`{field}\` element {i - 1} is not a finite number`)
    end
  end
end

local function validateEmitterProps(props: any)
  if type(props) ~= "table" then
    error("emitter: props must be a table")
  end

  -- sprite first so a missing sprite isn't masked by the lifetime check.
  if type(props.sprite) ~= "string" or #props.sprite == 0 then
    error("emitter: `sprite` must be a nonempty string")
  end

  if not isFiniteNumber(props.lifetime) or props.lifetime <= 0 then
    error("emitter: `lifetime` must be a number > 0")
  end

  if props.rate ~= nil then
    if not isFiniteNumber(props.rate) or props.rate < 0 then
      error("emitter: `rate` must be a number >= 0")
    end
  end

  if props.spread ~= nil then
    if not isFiniteNumber(props.spread) or props.spread < 0 then
      error("emitter: `spread` must be a number >= 0")
    end
  end

  if props.drag ~= nil then
    if not isFiniteNumber(props.drag) or props.drag < 0 then
      error("emitter: `drag` must be a number >= 0")
    end
  end

  if props.buoyancy ~= nil then
    if not isFiniteNumber(props.buoyancy) then
      error("emitter: `buoyancy` must be a finite number")
    end
  end

  if props.burst ~= nil then
    if not isFiniteNumber(props.burst) or props.burst < 0 or math.floor(props.burst) ~= props.burst then
      error("emitter: `burst` must be a non-negative integer")
    end
  end

  if props.spin_rate ~= nil then
    if not isFiniteNumber(props.spin_rate) then
      error("emitter: `spin_rate` must be a finite number")
    end
  end

  validateVec3(props.velocity, "velocity")

  if props.color ~= nil then
    validateVec3(props.color, "color")
    for i = 1, 3 do
      local c = props.color[i]
      if c < 0 or c > 1 then
        error(`emitter: \`color\` element {i - 1} ({c}) is outside [0, 1]`)
      end
    end
  end

  if props.size_over_lifetime ~= nil then
    validateCurve(props.size_over_lifetime, "size_over_lifetime")
  end

  if props.opacity_over_lifetime ~= nil then
    validateCurve(props.opacity_over_lifetime, "opacity_over_lifetime")
  end
end

local function defaulted(v: any, fallback: any): any
  if v == nil then
    return fallback
  end
  return v
end

-- Shallow merge: every key in `b` overrides the corresponding key in
-- `a`. Non-`nil` values win; `nil` overrides do not erase preset values
-- (Luau tables can't carry explicit `nil` slots anyway).
local function mergeProps(a: EmitterProps, b: any): EmitterProps
  local out: any = {}
  for k, v in pairs(a) do
    out[k] = v
  end
  if type(b) == "table" then
    for k, v in pairs(b) do
      out[k] = v
    end
  end
  return out :: EmitterProps
end

-- ---------------------------------------------------------------------
-- emitter() — validating component constructor
-- ---------------------------------------------------------------------

--- Build a `BillboardEmitter` component descriptor from `props`.
--- Validates synchronously and raises an error naming the offending
--- field on failure. Fills defaults for omitted optional fields:
---
--- - `rate = 0.0` (dormant continuous emission)
--- - `burst = nil` (no one-shot burst)
--- - `spread = 0.2` rad (~11° cone)
--- - `buoyancy = 0.5` (rises gently — smoke default)
--- - `drag = 0.5`
--- - `opacity_over_lifetime = {1.0, 1.0, 0.8, 0.0}` (full → fade-out)
--- - `size_over_lifetime = {1.0}` (constant)
--- - `color = {1.0, 1.0, 1.0}` (white tint)
--- - `spin_rate = 0.0` (no rotation)
---
--- `rate = 0` and `burst = nil` together describe a dormant emitter
--- — that is a valid configuration, not a validation error.
function EmittersSdk.emitter(props: EmitterProps): ComponentDescriptor
  validateEmitterProps(props)

  return {
    kind = "billboard_emitter",
    rate = defaulted(props.rate, 0.0),
    burst = props.burst,
    spread = defaulted(props.spread, 0.2),
    lifetime = props.lifetime,
    velocity = props.velocity,
    buoyancy = defaulted(props.buoyancy, 0.5),
    drag = defaulted(props.drag, 0.5),
    size_over_lifetime = defaulted(props.size_over_lifetime, {1.0}),
    opacity_over_lifetime = defaulted(props.opacity_over_lifetime, {1.0, 1.0, 0.8, 0.0}),
    color = defaulted(props.color, {1.0, 1.0, 1.0}),
    sprite = props.sprite,
    spin_rate = defaulted(props.spin_rate, 0.0),
  }
end

-- ---------------------------------------------------------------------
-- Presets
-- ---------------------------------------------------------------------

--- Soft, slowly-rising smoke. Use for chimneys, smoldering rubble,
--- incense pots — anything that needs a gentle vertical plume.
---
--- Defaults: continuous `rate: 6`/sec, `lifetime: 3s`, slight positive
--- `buoyancy: 0.2`, growing-while-fading curves, no spin. Sprite
--- collection is `"smoke"`.
function EmittersSdk.smokeEmitter(overrides: any?): ComponentDescriptor
  local defaults: EmitterProps = {
    rate = 6,
    lifetime = 3.0,
    buoyancy = 0.2,
    drag = 0.5,
    spread = 0.2,
    size_over_lifetime = {0.3, 1.5},
    opacity_over_lifetime = {0.0, 0.8, 0.6, 0.0},
    sprite = "smoke",
    spin_rate = 0.0,
    velocity = {0, 0.5, 0},
    color = {1.0, 1.0, 1.0},
  }
  return EmittersSdk.emitter(mergeProps(defaults, overrides))
end

--- Fast, falling, tumbling sparks. One-shot `burst: 12` per trigger;
--- `rate: 0` so the emitter is idle until a burst is requested.
---
--- Defaults: `lifetime: 0.6s`, gravity-pulled (`buoyancy: -1.0`), wide
--- `spread: 0.5`, shrinking-while-fading curves, `spin_rate: 1.5` rad/s
--- for a visible tumble. Warm orange tint. Sprite collection is
--- `"spark"`.
function EmittersSdk.sparkEmitter(overrides: any?): ComponentDescriptor
  local defaults: EmitterProps = {
    rate = 0,
    burst = 12,
    lifetime = 0.6,
    buoyancy = -1.0,
    drag = 0.1,
    spread = 0.5,
    size_over_lifetime = {1.0, 0.3},
    opacity_over_lifetime = {1.0, 1.0, 0.0},
    sprite = "spark",
    spin_rate = 1.5,
    velocity = {0, 2.0, 0},
    color = {1.0, 0.8, 0.3},
  }
  return EmittersSdk.emitter(mergeProps(defaults, overrides))
end

--- Slow drifting dust motes. Use for shafts of light, disturbed
--- floors, and ambient atmospheric particles.
---
--- Defaults: continuous `rate: 2`/sec, long `lifetime: 5s`,
--- near-neutral `buoyancy: 0.05` with high `drag: 1.0` so motes settle
--- quickly, subtle opacity bell, no spin. Warm-grey tint. Sprite
--- collection is `"dust"`.
function EmittersSdk.dustEmitter(overrides: any?): ComponentDescriptor
  local defaults: EmitterProps = {
    rate = 2,
    lifetime = 5.0,
    buoyancy = 0.05,
    drag = 1.0,
    spread = 0.3,
    size_over_lifetime = {0.5, 1.0},
    opacity_over_lifetime = {0.0, 0.3, 0.0},
    sprite = "dust",
    spin_rate = 0.0,
    velocity = {0, 0.1, 0},
    color = {0.8, 0.7, 0.6},
  }
  return EmittersSdk.emitter(mergeProps(defaults, overrides))
end

return EmittersSdk
