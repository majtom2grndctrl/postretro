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
-- `punch` (1,100 ms) connects at its `hit`, 267 ms in; the rest is
-- follow-through.
-- `smash` (3,700 ms) crouches by 400 ms and both hands reach the floor at its
-- `hit`, 1,667 ms in. The slam's phases share one smash; the commit holds
-- 100 ms past `hit`.
local SMASH_CROUCHED_MS = 400
local SMASH_HIT_MS = 1667
local SLAM_WINDUP_MS = SMASH_CROUCHED_MS
local SLAM_COMMIT_MS = SMASH_HIT_MS - SMASH_CROUCHED_MS + 100
-- A jab re-entered the tick after its fire rests, then plays a fresh punch to
-- `hit`: 27 ticks (450 ms) at the soonest. The cooldown sits below rest plus
-- `hit` (417 ms), so no jab's moment meets it.
local JAB_REST_MS = 150
local JAB_COOLDOWN_MS = 400
-- Longer than `heavy`'s 4 s refresh, so a second brute takes the next slam.
-- An outer row: passing during a jab's punch, it cuts that jab before `hit`.
local PRESS_MS = 5000

-- `and` is a Luau keyword, so composition uses bracket spelling; bind the left
-- operands first so a three-term guard does not repeat its prefix.
local inReach = brain.targetDistance:le(JAB_RANGE)
local restedInReach = inReach["and"](inReach, brain.timeInActivityMs:ge(JAB_REST_MS))
local inSlamRange = brain.targetDistance:le(SLAM_RANGE)
local windupDone = brain.timeInActivityMs:ge(SLAM_WINDUP_MS)
local noTarget = brain.hasTarget["not"](brain.hasTarget)
local staleNoTarget = noTarget["and"](noTarget, brain.timeSinceDamageMs:gt(ALERT_MS))
local freshKnownDamage = brain.damageSourceKnown["and"](brain.damageSourceKnown, brain.timeSinceDamageMs:le(STARTLE_MS))
local alertKnownDamage = brain.damageSourceKnown["and"](brain.damageSourceKnown, brain.timeSinceDamageMs:le(ALERT_MS))

local aheadBearing = brain.damageBearing:gt(HALF_PI)
local behindBearing = aheadBearing["or"](aheadBearing, brain.damageBearing:lt(-HALF_PI))

-- The jab loop both fighting phases nest.
local jabs = {
  initial = "stalk",
  activities = {
    stalk = { animation = "walk" },
    jab = {
      animation = "attack_jab",
      action = { attack = "jab", at = "hit" },
      attackTurn = "jab",
    },
  },
  transitions = {
    stalk = {
      {
        to = "jab",
        when = restedInReach["and"](restedInReach, attackTurnFree("jab")),
      },
    },
    jab = {
      { to = "stalk", when = brain.animationDone },
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
        -- The smash's hands reach the floor at its `hit`, 1,667 ms in, during
        -- the slam's commit. Stances split it into `smash_windup` and
        -- `smash_strike`.
        attack_slam = {
          clip = "smash",
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
        jab = {
          draws = { AttackTokens.melee },
          damage = 8,
          maxRange = JAB_RANGE,
          cooldownMs = JAB_COOLDOWN_MS,
        },
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
                -- The slam stands its ground through the whole smash rather
                -- than chasing, so the brute does not walk off its target
                -- before `hit`.
                windup = {
                  animation = "attack_slam",
                  motion = "hold",
                  attackTurn = "slam",
                },
                commit = {
                  animation = "attack_slam",
                  motion = "hold",
                  action = { attack = "slam", at = "hit" },
                  attackTurn = "slam",
                },
                recover = {
                  animation = "attack_slam",
                  motion = "hold",
                  attackTurn = "slam",
                },
              },
              transitions = {
                hunt = {
                  {
                    to = "windup",
                    when = inSlamRange["and"](inSlamRange, attackTurnFree("slam")),
                  },
                },
                windup = {
                  {
                    to = "commit",
                    when = windupDone["and"](windupDone, attackTurnHeld("slam")),
                  },
                  -- Another brute asked on the same tick and won the turn.
                  { to = "hunt", when = brain.timeInActivityMs:ge(SLAM_WINDUP_MS) },
                },
                commit = {
                  { to = "recover", when = brain.timeInActivityMs:ge(SLAM_COMMIT_MS) },
                },
                recover = { { to = "press", when = brain.animationDone } },
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
            when = staleNoTarget["and"](staleNoTarget, brain.distanceToLastKnown:le(ARRIVE)),
          },
          -- A selected neutral target is an engine-admitted over-tolerance
          -- retaliator, so it remains engageable like an ordinary hostile.
          {
            to = "startle",
            when = freshKnownDamage["and"](freshKnownDamage, behindBearing),
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
            when = alertKnownDamage["and"](alertKnownDamage, brain.distanceToLastKnown:gt(ARRIVE)),
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
