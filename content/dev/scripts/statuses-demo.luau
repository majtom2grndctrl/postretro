-- Luau twin of statuses-demo.ts: the statuses Scripting surface example as a
-- standalone mod manifest, mirroring damage-types.ts (fire, acid),
-- statuses.ts and status-hazards.ts. content/dev's own manifest is
-- TypeScript only, so this twin is where the Luau half runs;
-- `startup::status_content_tests` checks both produce the same manifest, and
-- `sim::status_dev_fixture_tests` drives both through the fixed tick. Every
-- definer is named, because Luau has no binding-name sugar. No mod or map
-- loads this file.
-- See: docs/scripting-reference.md (Statuses)

local Postretro = require("postretro")

local fire = Postretro.defineDamageType("fire", { health = 1.25 })
local acid = Postretro.defineDamageType("acid", { health = 0.75 })

local burning = Postretro.defineStatus("burning", {
  damage = 4,
  type = fire,
  tickMs = 500,
  durationMs = 3000,
  stacking = "stack",
  maxStacks = 3,
  presentation = {
    emitter = Postretro.emitter({
      sprite = "plasma_bolt",
      rate = 24,
      lifetime = 0.6,
      velocity = { 0, 1.5, 0 },
      spread = 0.4,
      buoyancy = 0.5,
      size_over_lifetime = { 0.35, 0.1 },
      opacity_over_lifetime = { 1, 0 },
      color = { 1, 0.45, 0.1 },
    }),
    tickSound = "fixtures/pellet_hit",
  },
})
local scalding = Postretro.defineStatus("scalding", {
  damage = 10,
  type = fire,
  tickMs = 250,
  durationMs = 250,
  presentation = { tickSound = "fixtures/bite" },
})
local corroding = Postretro.defineStatus("corroding", { damage = 2, type = acid, tickMs = 1000 })
local wet = Postretro.defineStatus("wet", { damage = 0, tickMs = 500, durationMs = 5000 })

-- Fire ignites what is not wet; acid dries it; a burn kill plays `burn_death`
-- where the target's animation set defines it (none in the dev mod does, so
-- `enemy-death` still plays `death`). The `impact.status(burning)` guard keeps
-- a burn tick from reapplying `burning`.
local ignite = Postretro.defineImpactEvent("statuses.ignite", { tag = "enemy" }, function(impact)
  local target = impact.target
  local isFire = impact.type(fire)
  local burnTick = impact.status(burning)
  local dry = target:hasStatus(wet)
  local lights = isFire["and"](isFire, burnTick["not"](burnTick))
  lights = lights["and"](lights, dry["not"](dry))
  return {
    { when = lights, ["do"] = { target:applyStatus(burning) } },
    { when = impact.type(acid), ["do"] = { target:clearStatus(wet) } },
    { when = burnTick["and"](burnTick, target.healthAfter:le(0)), ["do"] = { target:playAnim("burn_death") } },
  }
end)

-- Clears every player's corrosion.
local cure = Postretro.defineReaction("statuses.cure", Postretro.players():clearStatus(corroding))

-- A brain transition: flee while burning.
local _panic = { to = "flee", when = Postretro.hasStatus(burning) }

return Postretro.defineMod({
  name = "statuses-demo",
  id = "example.statuses",
  version = "1",
  damageTypes = { fire, acid },
  statuses = { burning, scalding, corroding, wet },
  fluids = {
    Postretro.defineFluid({
      name = "lava",
      viscosity = 0.6,
      viewTint = { 1, 0.35, 0.05, 0.8 },
      muffle = 0.9,
      surfaceOpacity = 1,
      status = scalding,
    }),
  },
  events = { ignite },
  reactions = { cure },
})
