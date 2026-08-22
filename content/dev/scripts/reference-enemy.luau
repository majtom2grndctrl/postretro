--!strict
-- The dev mod's hand-authored Luau twin of `reference-enemy.ts`.

local M = {}

M.REFERENCE_ENEMY_CLASSNAME = "reference_enemy"

local DETECTION_RANGE = 16
local JAB_RANGE = 2
local SLAM_RANGE = 3.5
local LEASH_RANGE = 100
local RETURN_ARRIVAL_EPSILON = 1
local SLAM_WINDUP_MS = 250
local SLAM_COMMIT_MS = 150
-- The 1,850 ms phase cycle is longer than slam's 1,800 ms cooldown.
local SLAM_RECOVER_MS = 1450

M.referenceEnemyEntity = defineEntity({
  canonicalName = M.REFERENCE_ENEMY_CLASSNAME,
  components = {
    health = {
      max = 70,
      hitbox = {
        halfExtents = { 0.4, 0.9, 0.4 },
        offset = { 0, 0.9, 0 },
      },
      zoneMultipliers = {
        head = 2.5,
        leg = 0.5,
      },
    },
    mesh = {
      model = "models/reference_enemy_kaykit_knight/scene.gltf",
      animations = {
        idle = { clip = "Idle", loop = true },
        walk = {
          clip = "Walking_A",
          loop = true,
          crossfadeMs = 200,
          travelSpeed = 3,
        },
        attack_jab = {
          clip = "1H_Melee_Attack_Slice_Horizontal",
          loop = false,
          crossfadeMs = 80,
          interrupt = "snap",
        },
        attack_slam = {
          clip = "1H_Melee_Attack_Slice_Horizontal",
          loop = false,
          crossfadeMs = 80,
          interrupt = "snap",
        },
        death = {
          clip = "Death_A",
          loop = false,
          crossfadeMs = 120,
          interrupt = "snap",
        },
      },
      defaultState = "idle",
      locomotion = { speedScale = true },
    },
    behavior = {
      initial = "idle",
      moveSpeed = 3,
      attacks = {
        jab = { damage = 8, maxRange = JAB_RANGE, cooldownMs = 1200 },
        slam = {
          damage = 14,
          maxRange = SLAM_RANGE,
          cooldownMs = 1800,
          engagementRadius = SLAM_RANGE,
        },
      },
      engagementRadius = JAB_RANGE,
      patrol = {
        mode = "pingPong",
        points = { { 0, 0 }, { 6, 0 }, { 6, 6 } },
      },
      activities = {
        idle = { animation = "idle", motion = "hold" },
        patrol = { animation = "walk", motion = "patrol" },
        engage = {
          animation = "walk",
          layers = {
            move = {
              {
                when = brain.targetDistance:le(SLAM_RANGE),
                motion = "hold",
              },
              "chaseTarget",
            },
            offense = {
              initial = "approach",
              activities = {
                approach = { animation = "walk" },
                jab = {
                  animation = "attack_jab",
                  action = { attack = "jab" },
                },
                windup = { animation = "attack_slam" },
                commit = {
                  animation = "attack_slam",
                  action = { attack = "slam" },
                },
                recover = { animation = "attack_slam" },
              },
              transitions = {
                approach = {
                  { to = "jab", when = brain.targetDistance:le(JAB_RANGE) },
                  { to = "windup", when = brain.targetDistance:le(SLAM_RANGE) },
                },
                jab = {
                  {
                    to = "windup",
                    when = brain.attacksFiredInActivity:ge(1),
                  },
                },
                windup = {
                  { to = "commit", when = brain.timeInActivityMs:ge(SLAM_WINDUP_MS) },
                },
                commit = {
                  { to = "recover", when = brain.timeInActivityMs:ge(SLAM_COMMIT_MS) },
                },
                recover = {
                  { to = "approach", when = brain.timeInActivityMs:ge(SLAM_RECOVER_MS) },
                },
              },
            },
          },
        },
        retreat = { animation = "walk", motion = "moveToAnchor" },
      },
      transitions = {
        ["*"] = {
          { to = "patrol", when = brain.hasTarget["not"](brain.hasTarget) },
          { to = "patrol", when = brain.targetHostile["not"](brain.targetHostile) },
        },
        idle = { { to = "patrol", when = runtime.constant(true) } },
        patrol = {
          {
            to = "engage",
            when = brain.acquisitionDue["and"](
              brain.acquisitionDue,
              brain.targetDistance:le(DETECTION_RANGE)
            ),
          },
        },
        engage = {
          { to = "retreat", when = brain.distanceFromAnchor:gt(LEASH_RANGE) },
        },
        retreat = {
          { to = "patrol", when = brain.distanceFromAnchor:le(RETURN_ARRIVAL_EPSILON) },
        },
      },
    },
  },
})

return M
