--!strict
-- The dev mod's hand-authored Luau twin of `reference-enemy.ts`.

local M = {}

local AttackTokens = require("./scripts/attack-tokens.luau")
local DamageTypes = require("./scripts/damage-types.luau")
local Defenses = require("./scripts/defenses.luau")

M.REFERENCE_ENEMY_CLASSNAME = "reference_enemy"

local DETECTION_RANGE = 16
local ALERT_MS = 4000
local SEARCH_AFTER_MS = 800
local ARRIVE = 1
local HALF_PI = math.pi / 2
local STARTLE_MS = 200
local JAB_RANGE = 2
local SLAM_RANGE = 3.5
-- Combat slots sit inside jab reach, so the brute fights close enough to jab.
local CLOSE_STANDOFF = 1.5
local LEASH_RANGE = 100
local RETURN_ARRIVAL_EPSILON = 1
local SLAM_WINDUP_MS = 250
local SLAM_COMMIT_MS = 150
-- The 1,850 ms phase cycle is longer than slam's 1,800 ms cooldown.
local SLAM_RECOVER_MS = 1450
-- Strike plus rest covers the jab's 1,200 ms cooldown.
local JAB_STRIKE_MS = 450
local JAB_REST_MS = 750
-- Longer than `heavy`'s 4 s refresh, so a second brute takes the next slam.
local PRESS_MS = 5000

-- The jab loop both fighting phases nest.
local jabs = {
  initial = "stalk",
  activities = {
    stalk = { animation = "walk" },
    jab = {
      animation = "attack_jab",
      action = { attack = "jab" },
      attackTurn = "jab",
    },
  },
  transitions = {
    stalk = {
      {
        to = "jab",
        when = brain.targetDistance:le(JAB_RANGE)["and"](
          brain.targetDistance:le(JAB_RANGE),
          brain.timeInActivityMs:ge(JAB_REST_MS)
        )["and"](
          brain.targetDistance:le(JAB_RANGE)["and"](
            brain.targetDistance:le(JAB_RANGE),
            brain.timeInActivityMs:ge(JAB_REST_MS)
          ),
          attackTurnFree("jab")
        ),
      },
    },
    jab = {
      {
        to = "stalk",
        when = brain.attacksFiredInActivity:ge(1)["and"](
          brain.attacksFiredInActivity:ge(1),
          brain.timeInActivityMs:ge(JAB_STRIKE_MS)
        ),
      },
      { to = "stalk", when = brain.targetDistance:gt(JAB_RANGE) },
    },
  },
}

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
      -- Armored, and its flesh shock-resistant (health only).
      layers = { { layer = Defenses.armor, capacity = 120 } },
      resist = { shock = 0.5 },
    },
    -- A single normal hit is enough to redirect this playtest fixture.
    tolerance = 4,
    -- The Mixamo Mutant, on the `mixamo_brute` clip library.
    mesh = {
      model = "models/mutant/model.gltf",
      animations = {
        idle = { clip = "idle", loop = true },
        -- Every move runs at `moveSpeed`, so locomotion plays the run; its
        -- planted foot travels about 2.2 m/s.
        walk = {
          clip = "run",
          loop = true,
          crossfadeMs = 200,
          travelSpeed = 2.2,
        },
        attack_jab = {
          clip = "punch",
          loop = false,
          crossfadeMs = 80,
          interrupt = "snap",
        },
        -- The punch's strike lands about 250 ms in, with the slam's commit.
        -- Stances replace this cycle with `smash` and `swipe`.
        attack_slam = {
          clip = "punch",
          loop = false,
          crossfadeMs = 80,
          interrupt = "snap",
        },
        flinch = {
          clip = "hit_reaction",
          loop = false,
          crossfadeMs = 80,
          interrupt = "snap",
        },
        death = {
          clip = "death",
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
        jab = { draws = { AttackTokens.melee }, damage = 8, maxRange = JAB_RANGE, cooldownMs = 1200 },
        slam = {
          draws = { AttackTokens.melee:with({ cost = 2 }), AttackTokens.heavy },
          damage = 14,
          maxRange = SLAM_RANGE,
          cooldownMs = 1800,
        },
        claw = { damage = 15, damageType = DamageTypes.corrosive, maxRange = 2, cooldownMs = 800 },
      },
      engagementRadius = CLOSE_STANDOFF,
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
                when = brain.targetDistance:le(JAB_RANGE),
                motion = "hold",
              },
              "chaseTarget",
            },
            offense = {
              initial = "hunt",
              activities = {
                hunt = { layers = { offense = jabs } },
                press = { layers = { offense = jabs } },
                windup = { animation = "attack_slam", attackTurn = "slam" },
                commit = {
                  animation = "attack_slam",
                  action = { attack = "slam" },
                  attackTurn = "slam",
                },
                recover = { animation = "attack_slam", attackTurn = "slam" },
              },
              transitions = {
                hunt = {
                  {
                    to = "windup",
                    when = brain.targetDistance:le(SLAM_RANGE)["and"](
                      brain.targetDistance:le(SLAM_RANGE),
                      attackTurnFree("slam")
                    ),
                  },
                },
                windup = {
                  {
                    to = "commit",
                    when = brain.timeInActivityMs:ge(SLAM_WINDUP_MS)["and"](
                      brain.timeInActivityMs:ge(SLAM_WINDUP_MS),
                      attackTurnHeld("slam")
                    ),
                  },
                  -- Another brute asked on the same tick and won the turn.
                  { to = "hunt", when = brain.timeInActivityMs:ge(SLAM_WINDUP_MS) },
                },
                commit = {
                  { to = "recover", when = brain.timeInActivityMs:ge(SLAM_COMMIT_MS) },
                },
                recover = {
                  { to = "press", when = brain.timeInActivityMs:ge(SLAM_RECOVER_MS) },
                },
                press = { { to = "hunt", when = brain.timeInActivityMs:ge(PRESS_MS) } },
              },
            },
          },
        },
        investigate = { animation = "walk", motion = "moveToLastKnown" },
        startle = { animation = "flinch", motion = "hold" },
        retreat = { animation = "walk", motion = "moveToAnchor" },
      },
      transitions = {
        ["*"] = {
          -- A missing target is not enough to stand down: recent damage can
          -- still point at an unreached memory.
          {
            to = "patrol",
            when = brain.hasTarget["not"](brain.hasTarget)["and"](
              brain.hasTarget["not"](brain.hasTarget)["and"](
                brain.hasTarget["not"](brain.hasTarget),
                brain.timeSinceDamageMs:gt(ALERT_MS)
              ),
              brain.distanceToLastKnown:le(ARRIVE)
            ),
          },
          -- A selected neutral target is an engine-admitted over-tolerance
          -- retaliator, so it remains engageable like an ordinary hostile.
          {
            to = "startle",
            when = brain.damageSourceKnown["and"](
              brain.damageSourceKnown,
              brain.timeSinceDamageMs:le(STARTLE_MS)
            )["and"](
              brain.damageSourceKnown["and"](
                brain.damageSourceKnown,
                brain.timeSinceDamageMs:le(STARTLE_MS)
              ),
              brain.damageBearing:gt(HALF_PI)["or"](
                brain.damageBearing:gt(HALF_PI),
                brain.damageBearing:lt(-HALF_PI)
              )
            ),
          },
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
          {
            to = "investigate",
            -- Generic damage recency also resets for contextless reactions;
            -- provenance proves this hit supplied the remembered position.
            when = brain.damageSourceKnown["and"](
              brain.damageSourceKnown,
              brain.timeSinceDamageMs:le(ALERT_MS)
            )["and"](
              brain.damageSourceKnown["and"](
                brain.damageSourceKnown,
                brain.timeSinceDamageMs:le(ALERT_MS)
              ),
              brain.distanceToLastKnown:gt(ARRIVE)
            ),
          },
        },
        engage = {
          { to = "retreat", when = brain.distanceFromAnchor:gt(LEASH_RANGE) },
          -- Its own layer, as `state(name)` reads its own state.
          { to = "retreat", when = defense(Defenses.armor).current:le(0) },
          {
            to = "investigate",
            when = brain.timeSinceTargetVisible:ge(SEARCH_AFTER_MS),
          },
        },
        investigate = {
          { to = "engage", when = brain.targetVisible },
          { to = "patrol", when = brain.distanceToLastKnown:le(ARRIVE) },
        },
        startle = {
          { to = "patrol", when = brain.timeInActivityMs:ge(STARTLE_MS) },
        },
        retreat = {
          { to = "patrol", when = brain.distanceFromAnchor:le(RETURN_ARRIVAL_EPSILON) },
        },
      },
    },
  },
})

return M
