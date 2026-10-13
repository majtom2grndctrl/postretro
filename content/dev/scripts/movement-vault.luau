-- Luau twin of movement-vault.ts: the vault scripting-surface example as a
-- standalone mod manifest. content/dev's own manifest is TypeScript only, so
-- this twin is where the Luau half runs; `startup::movement_vault_surface_tests`
-- checks both produce the same manifest. No mod or map loads this file.
-- See: docs/scripting-reference.md (movement.vault)

local Postretro = require("postretro")

return Postretro.defineMod({
  name = "movement-vault-fixture",
  id = "postretro.dev-movement-vault",
  version = "dev",
  entities = {
    Postretro.defineEntity({
      canonicalName = "player",
      components = {
        movement = {
          capsule = { radius = 0.2, halfHeight = 0.8, eyeHeight = 0.5 },
          ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 15.0, stepHeight = 0.5, maxSlope = 45.0 },
          air = { forwardSteer = 0.5, accel = 10, maxControlSpeed = 2, bunnyHop = true, jumps = 0, jumpVelocity = 6.86, jumpCeiling = 0.0 },
          fall = { terminalVelocity = 40.0 },
          -- Absent = never vaults.
          vault = {
            maxHeight = 1.6, -- m above the feet; above stepHeight
            swimMaxHeight = 0.9, -- m above the fluid surface
            durationMs = 800, -- rise-and-over time
            from = {
              -- place → "auto" | "jump"; a missing place cannot vault
              air = "auto",
              swim = "auto", -- omit for Quake-style fluid exits only
              -- ground = "jump", -- opt in: jump onto a ledge from standing
            },
            allowFire = false, -- hands on the ledge
          },
        },
      },
    }),
  },
})
