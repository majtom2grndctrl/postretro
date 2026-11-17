--!strict
-- The dev mod's hand-authored Luau twin of `stance-surface.ts`: the Stances
-- section of docs/scripting-reference.md as dev content.
-- Luau names each declaration explicitly.

local M = {}

local AttackTokens = require("./scripts/attack-tokens.luau")

M.STANCE_SURFACE_CLASSNAME = "stance_surface_brute"

-- Poses.
local overhead = defineStance("overhead", {
  animation = "smash_windup",
  -- Stance sounds have no assets yet; a dev fixture stands in.
  sound = "fixtures/growl",
  holdMs = { 500, 1200 },
  breakOn = { damage = 25, to = "flinch" },
})
local coil = defineStance("coil", { animation = "swipe_windup" })
local bellow = defineStance("bellow", { animation = "roar" })

-- Strikes. Two begin in `overhead`: same pose, different hit.
local slam = defineAttack("slam", {
  draws = { AttackTokens.melee:with({ cost = 2 }), AttackTokens.heavy },
  damage = 14,
  maxRange = 3.5,
  cooldownMs = 1800,
  beginningStance = overhead,
  endingStance = coil,
  release = { animation = "smash_strike", at = "hit" },
})
local sweep = defineAttack("sweep", {
  draws = { AttackTokens.melee },
  damage = 8,
  maxRange = 3.5,
  cooldownMs = 1200,
  beginningStance = overhead,
  release = { animation = "swipe_strike" },
})
local lunge = defineAttack("lunge", {
  draws = { AttackTokens.melee },
  damage = 10,
  maxRange = 3.5,
  cooldownMs = 1500,
  beginningStance = coil,
  release = { animation = "swipe_strike", at = "hit" },
})
local charge = defineAttack("charge", {
  draws = { AttackTokens.heavy },
  damage = 18,
  maxRange = 12,
  cooldownMs = 4000,
  beginningStance = bellow,
  release = { animation = "run", motion = "chaseTarget", speed = 2, ms = 1500, at = false },
  recover = { animation = "idle", ms = 600 },
})

local noTarget = brain.hasTarget["not"](brain.hasTarget)

M.stanceSurfaceEntity = defineEntity({
  canonicalName = M.STANCE_SURFACE_CLASSNAME,
  components = {
    health = {
      max = 70,
      hitbox = { halfExtents = { 0.4, 0.9, 0.4 }, offset = { 0, 0.9, 0 } },
    },
    mesh = {
      model = "models/mutant/model.gltf",
      animations = {
        idle = { clip = "idle", loop = true },
        walk = { clip = "walk", loop = true, crossfadeMs = 200 },
        run = { clip = "run", loop = true, crossfadeMs = 120, travelSpeed = 2.2 },
        smash_windup = { clip = "smash_windup", loop = false, crossfadeMs = 80 },
        swipe_windup = { clip = "swipe_windup", loop = false, crossfadeMs = 80 },
        roar = { clip = "roar", loop = false, crossfadeMs = 80 },
        smash_strike = { clip = "smash_strike", loop = false, interrupt = "snap" },
        swipe_strike = { clip = "swipe_strike", loop = false, interrupt = "snap" },
        flinch = { clip = "hit_reaction", loop = false, crossfadeMs = 80, interrupt = "snap" },
      },
      defaultState = "idle",
      locomotion = { speedScale = true },
    },
    behavior = {
      initial = "idle",
      moveSpeed = 3,
      activities = {
        idle = { animation = "idle", motion = "hold" },
        engage = {
          animation = "walk",
          layers = {
            move = { "chaseTarget" },
            offense = {
              initial = "approach",
              activities = {
                approach = { animation = "walk" },
                flinch = { animation = "flinch", motion = "hold" },
              },
              transitions = {
                approach = {
                  { to = overhead, when = brain.targetDistance:le(3.5) },
                  { to = bellow, when = brain.targetDistance:ge(8) },
                },
                overhead = {
                  { to = slam, when = brain.activityRoll:lt(0.5) },
                  { to = sweep },
                  { to = "approach", when = brain.targetDistance:gt(4.5) },
                },
                coil = {
                  { to = lunge },
                  { to = "approach", when = brain.timeInActivityMs:ge(800) },
                },
                bellow = {
                  { to = charge },
                  { to = "approach", when = brain.timeInActivityMs:ge(1500) },
                },
                flinch = { { to = "approach", when = brain.timeInActivityMs:ge(450) } },
              },
            },
          },
        },
      },
      transitions = {
        idle = { { to = "engage", when = brain.hasTarget } },
        engage = { { to = "idle", when = noTarget } },
      },
    },
  },
})

return M
