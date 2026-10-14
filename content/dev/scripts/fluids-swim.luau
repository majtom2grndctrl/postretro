-- Luau twin of fluids-swim.ts: the fluids-and-swimming scripting-surface
-- example as a standalone mod manifest. content/dev's own manifest is
-- TypeScript only, so this twin is where the Luau half runs;
-- `startup::fluids_swim_surface_tests` checks both produce the same manifest.
-- Every reaction is named, because derived ids differ per runtime. No mod or
-- map loads this file.
-- See: docs/scripting-reference.md (Fluids and swimming)

local Postretro = require("postretro")
local UI = require("postretro/ui")

local player = Postretro.getGameState().player
local read = Postretro.read

-- Mirrors swim-events.ts.
local swimStats = Postretro.defineStore("swimStats", {
  dives = { type = "number", default = 0, perOwner = true },
})
local markSwimming = Postretro.defineReaction("swim.markSwimming", function(on)
  return on.player:addSlot(swimStats.dives, 1)
end)
local splashSound = Postretro.defineReaction("swim.splash", UI.playSound("fixtures/pellet_hit"))

return Postretro.defineMod({
  name = "fluids-swim-fixture",
  id = "postretro.dev-fluids-swim",
  version = "dev",
  fluids = {
    Postretro.defineFluid({
      name = "water",
      viscosity = 2.0, -- 1/s drag while swimming; >= 0
      viewTint = { 0.1, 0.3, 0.4, 0.35 }, -- rgb + strength, each 0..=1
      muffle = 0.8, -- 0 = none, 1 = maximum low-pass
      surfaceOpacity = 0.75, -- 0..=1
      enterSound = "fixtures/land", -- optional
      exitSound = "fixtures/jump", -- optional
    }),
    Postretro.defineFluid({
      name = "sludge",
      viscosity = 6.0,
      viewTint = { 0.25, 0.3, 0.08, 0.85 },
      muffle = 0.95,
      surfaceOpacity = 0.95,
      enterSound = "fixtures/land",
    }),
  },
  entities = {
    Postretro.defineEntity({
      canonicalName = "player",
      components = {
        movement = {
          capsule = { radius = 0.2, halfHeight = 0.8, eyeHeight = 0.5 },
          ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 15.0, stepHeight = 0.5, maxSlope = 45.0 },
          air = { forwardSteer = 0.5, accel = 10, maxControlSpeed = 2, bunnyHop = true, jumps = 0, jumpVelocity = 6.86, jumpCeiling = 0.0 },
          fall = { terminalVelocity = 40.0 },
          -- Absent = cannot swim.
          swim = {
            speed = 4.5, -- m/s target swim speed
            accel = 6.0, -- m/s² toward wish velocity
            sinkSpeed = 0.8, -- m/s drift along gravity with no vertical input, at default gravity
            enterDepth = 0.5, -- immersion fraction that enters swim
            exitDepth = 0.35, -- immersion fraction that leaves swim; < enterDepth
            fluidJumpVelocity = 9.35, -- m/s upward on a jump from the surface
          },
        },
      },
    }),
  },
  stores = { swimStats },
  reactions = { markSwimming, splashSound },
  -- A host decision per player.
  playerEvents = { Postretro.players():on(Postretro.becomes(read(player.swimming)), { markSwimming }) },
  -- Own-state feedback stays local on each machine.
  crossings = { UI.onStateCrossing(read(player.swimming), { splashSound }) },
})
