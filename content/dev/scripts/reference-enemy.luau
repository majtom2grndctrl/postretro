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
local LUNGE_RANGE = 3
local SLAM_RANGE = 3.5
-- The charge's `maxRange` both opens its queue and lands its contact.
local CHARGE_REACH = 2.5
-- Combat slots sit inside jab reach, so the brute fights close enough to jab.
local CLOSE_STANDOFF = 1.5
-- The leap's tell waits in line from this far out; its hit lands at slam range.
local LEAP_QUEUE_RANGE = 50
-- The leap's top pace, a multiple of `moveSpeed`. See the leap below.
local LEAP_MAX_SPEED = 14
-- The combo holds its drawn fist this long before the jabs, long enough to
-- read as a wind-up and not a stutter.
local FLURRY_MS = 2000
-- The leap holds its crouch this long.
local CROUCH_MS = 666
local LEASH_RANGE = 100
local RETURN_ARRIVAL_EPSILON = 1
-- Strolls on the walk at its planted-foot speed, 1.2 m/s.
local STROLL = 0.4
-- `approach`'s tell, from one roll per entry: below 0.4 the crouch at slam
-- range, below 0.25 the leaping crouch beyond it, 0.85 and up the roar,
-- 0.65 to 0.85 the combo's drawn fist, else the cocked fist; any other roll
-- at slam range presses in.
local OVERHEAD_SHARE = 0.4
local LEAP_SHARE = 0.25
local BELLOW_FROM = 0.85
local FLURRY_FROM = 0.65
-- `press`'s fresh roll, about `approach`'s shares: 0.75 and up the roar, 0.4
-- to 0.75 the drawn fist, else the cocked fist. Held short of jab reach this
-- long, it takes the crouch, whose strikes reach farther than the jab.
local PRESS_BELLOW_FROM = 0.75
local PRESS_FLURRY_FROM = 0.4
local PRESS_STALL_MS = 3000
-- The crouch's two strikes.
local SLAM_SHARE = 0.5
local OVERHEAD_WAIT_MS = 3000
local COCK_WAIT_MS = 2000
local BELLOW_WAIT_MS = 2500
local OVERHEAD_LEAVE_RANGE = SLAM_RANGE + 1
local COCK_LEAVE_RANGE = JAB_RANGE + 1
local CROUCH_WAIT_MS = 3000
local CROUCH_LEAVE_RANGE = LEAP_QUEUE_RANGE
local COIL_MS = 500
local FLINCH_MS = 700

-- Poses. Stance sounds have no assets yet; dev fixtures stand in.
local overhead = defineStance("overhead", {
  animation = "smash_windup",
  sound = "fixtures/jump",
  holdMs = { 500, 1100 },
  breakOn = { damage = 30, to = "flinch" },
})
-- The slam's and the leap's landing pose; no strike begins here.
local coil = defineStance("coil", { animation = "swipe_windup", sound = "fixtures/land" })
local cock = defineStance("cock", {
  animation = "punch_windup",
  sound = "fixtures/bite",
  holdMs = { 400, 800 },
  breakOn = { damage = 20, to = "flinch" },
})
local bellow = defineStance("bellow", {
  animation = "roar",
  sound = "fixtures/growl",
  holdMs = { 800, 1400 },
  breakOn = { damage = 40, to = "flinch" },
})
-- The combo's tell: the cocked fist's pose, told apart by its sound.
local flurry = defineStance("flurry", {
  animation = "punch_windup",
  sound = "fixtures/pellet_hit",
  holdMs = FLURRY_MS,
  breakOn = { damage = 20, to = "flinch" },
})
-- The leap's tell: the slam's crouch, taken from afar.
local crouch = defineStance("crouch", {
  animation = "smash_windup",
  sound = "fixtures/jump",
  holdMs = CROUCH_MS,
  breakOn = { damage = 30, to = "flinch" },
})

-- Strikes, each firing at its clip's `hit` unasked; the charge on reach.
local jab = defineAttack("jab", {
  draws = { AttackTokens.melee },
  damage = 8,
  maxRange = JAB_RANGE,
  cooldownMs = 800,
  beginningStance = cock,
  endingStance = overhead,
  release = { animation = "punch_strike" },
})
local slam = defineAttack("slam", {
  draws = { AttackTokens.melee:with({ cost = 2 }), AttackTokens.heavy },
  damage = 14,
  maxRange = SLAM_RANGE,
  cooldownMs = 1800,
  beginningStance = overhead,
  endingStance = coil,
  release = { animation = "smash_strike" },
})
local lunge = defineAttack("lunge", {
  draws = { AttackTokens.melee },
  damage = 10,
  maxRange = LUNGE_RANGE,
  cooldownMs = 1500,
  beginningStance = overhead,
  endingStance = cock,
  release = { animation = "swipe_strike" },
})
-- It runs for a combat slot at jab reach, inside its contact reach.
local charge = defineAttack("charge", {
  draws = { AttackTokens.heavy },
  damage = 16,
  maxRange = CHARGE_REACH,
  standoffDistance = CLOSE_STANDOFF,
  cooldownMs = 5000,
  beginningStance = bellow,
  endingStance = cock,
  release = { animation = "run", motion = "chaseTarget", speed = 2, ms = 1200, at = false },
  recover = { animation = "idle", ms = 500 },
})
-- Three quick jabs, 320 ms apart; one cooldown from the last. The recover
-- leans back, a held pose that is not the drawn fist.
local combo = defineAttack("combo", {
  draws = { AttackTokens.melee },
  damage = 5,
  maxRange = JAB_RANGE,
  cooldownMs = 2400,
  beginningStance = flurry,
  endingStance = coil,
  release = { animation = "jab_combo", ["repeat"] = 3 },
  recover = { animation = "swipe_windup", ms = 400 },
})
-- Leaps from up to 50 m to a spot fixed on takeoff, its standoff short of the
-- target, paced to land at the clip's `hit` and never faster than 14x
-- `moveSpeed` (42 m/s); it slams at the `hit`, a whiff if the target is then
-- out of slam range.
local leap = defineAttack("leap", {
  draws = { AttackTokens.melee:with({ cost = 2 }), AttackTokens.heavy },
  damage = 18,
  maxRange = SLAM_RANGE,
  queueRange = LEAP_QUEUE_RANGE,
  standoffDistance = CLOSE_STANDOFF,
  cooldownMs = 6000,
  beginningStance = crouch,
  endingStance = coil,
  release = { animation = "leap_strike", motion = "leap", speed = LEAP_MAX_SPEED },
  recover = { animation = "idle", ms = 600 },
})
-- `claw` is not wired into the graph; the explicit `attacks` list keeps it on
-- the archetype.
local claw = defineAttack("claw", { damage = 15, damageType = DamageTypes.corrosive, maxRange = 2, cooldownMs = 800 })

-- `and` is a Luau keyword, so composition uses bracket spelling; bind the left
-- operands first so a three-term guard does not repeat its prefix.
local inSlamRange = brain.targetDistance:le(SLAM_RANGE)
local inChargeReach = brain.targetDistance:le(CHARGE_REACH)
local inLeapQueue = brain.targetDistance:le(LEAP_QUEUE_RANGE)
local inJabRange = brain.targetDistance:le(JAB_RANGE)
local noTarget = brain.hasTarget["not"](brain.hasTarget)
local staleNoTarget = noTarget["and"](noTarget, brain.timeSinceDamageMs:gt(ALERT_MS))
local freshKnownDamage = brain.damageSourceKnown["and"](brain.damageSourceKnown, brain.timeSinceDamageMs:le(STARTLE_MS))
local alertKnownDamage = brain.damageSourceKnown["and"](brain.damageSourceKnown, brain.timeSinceDamageMs:le(ALERT_MS))

local aheadBearing = brain.damageBearing:gt(HALF_PI)
local behindBearing = aheadBearing["or"](aheadBearing, brain.damageBearing:lt(-HALF_PI))

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
        walk = { clip = "walk", loop = true, crossfadeMs = 200, travelSpeed = 1.2 },
        -- Dev-content exception: the charge strides about 2.7x on `run`.
        -- Shipped content keeps the 1.5x default.
        run = { clip = "run", loop = true, crossfadeMs = 200, travelSpeed = 2.2, maxPlaybackRate = 3 },
        smash_windup = { clip = "smash_windup", loop = false, crossfadeMs = 80 },
        swipe_windup = { clip = "swipe_windup", loop = false, crossfadeMs = 80 },
        punch_windup = { clip = "punch_windup", loop = false, crossfadeMs = 80 },
        roar = { clip = "roar", loop = false, crossfadeMs = 120 },
        smash_strike = { clip = "smash_strike", loop = false, crossfadeMs = 80, interrupt = "snap" },
        swipe_strike = { clip = "swipe_strike", loop = false, crossfadeMs = 80, interrupt = "snap" },
        punch_strike = { clip = "punch_strike", loop = false, crossfadeMs = 80, interrupt = "snap" },
        jab_combo = {
          clip = "jab_quick",
          loop = true,
          crossfadeMs = 80,
          interrupt = "snap",
          playbackRate = 2.5,
        },
        leap_strike = {
          clip = "smash_strike",
          loop = false,
          crossfadeMs = 80,
          interrupt = "snap",
          scaleWithSpeed = false,
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
      attacks = { jab, slam, lunge, charge, combo, leap, claw },
      engagementRadius = CLOSE_STANDOFF,
      patrol = {
        mode = "pingPong",
        points = { { 0, 0 }, { 6, 0 }, { 6, 6 } },
      },
      activities = {
        idle = { animation = "idle", motion = "hold" },
        patrol = { animation = "walk", motion = "patrol", speed = STROLL },
        engage = {
          animation = "run",
          layers = {
            move = {
              {
                when = brain.targetDistance:le(JAB_RANGE),
                motion = "hold",
              },
              "chaseTarget",
            },
            offense = {
              initial = "approach",
              activities = {
                approach = { animation = "run" },
                press = { animation = "run" },
                flinch = { animation = "flinch", motion = "hold" },
              },
              transitions = {
                approach = {
                  {
                    to = overhead,
                    when = inSlamRange["and"](inSlamRange, brain.activityRoll:lt(OVERHEAD_SHARE)),
                  },
                  {
                    to = crouch,
                    when = inLeapQueue["and"](inLeapQueue, brain.activityRoll:lt(LEAP_SHARE)),
                  },
                  {
                    to = bellow,
                    when = inChargeReach["and"](inChargeReach, brain.activityRoll:ge(BELLOW_FROM)),
                  },
                  {
                    to = flurry,
                    when = inJabRange["and"](inJabRange, brain.activityRoll:ge(FLURRY_FROM)),
                  },
                  { to = cock, when = brain.targetDistance:le(JAB_RANGE) },
                  { to = "press", when = brain.targetDistance:le(SLAM_RANGE) },
                },
                press = {
                  { to = "approach", when = brain.targetDistance:gt(OVERHEAD_LEAVE_RANGE) },
                  {
                    to = bellow,
                    when = inChargeReach["and"](inChargeReach, brain.activityRoll:ge(PRESS_BELLOW_FROM)),
                  },
                  {
                    to = flurry,
                    when = inJabRange["and"](inJabRange, brain.activityRoll:ge(PRESS_FLURRY_FROM)),
                  },
                  { to = cock, when = brain.targetDistance:le(JAB_RANGE) },
                  {
                    to = overhead,
                    when = inSlamRange["and"](inSlamRange, brain.timeInActivityMs:ge(PRESS_STALL_MS)),
                  },
                },
                overhead = {
                  { to = slam, when = brain.activityRoll:lt(SLAM_SHARE) },
                  { to = lunge },
                  { to = "approach", when = brain.targetDistance:gt(OVERHEAD_LEAVE_RANGE) },
                  { to = "approach", when = brain.timeInActivityMs:ge(OVERHEAD_WAIT_MS) },
                },
                cock = {
                  { to = jab },
                  { to = "approach", when = brain.targetDistance:gt(COCK_LEAVE_RANGE) },
                  { to = "approach", when = brain.timeInActivityMs:ge(COCK_WAIT_MS) },
                },
                bellow = {
                  { to = charge },
                  { to = "approach", when = brain.timeInActivityMs:ge(BELLOW_WAIT_MS) },
                },
                flurry = {
                  { to = combo },
                  { to = "approach", when = brain.targetDistance:gt(COCK_LEAVE_RANGE) },
                  { to = "approach", when = brain.timeInActivityMs:ge(COCK_WAIT_MS) },
                },
                -- Can show while the leap cools, then waits out CROUCH_WAIT_MS.
                crouch = {
                  { to = leap },
                  { to = "approach", when = brain.targetDistance:gt(CROUCH_LEAVE_RANGE) },
                  { to = "approach", when = brain.timeInActivityMs:ge(CROUCH_WAIT_MS) },
                },
                coil = { { to = "approach", when = brain.timeInActivityMs:ge(COIL_MS) } },
                flinch = { { to = "approach", when = brain.timeInActivityMs:ge(FLINCH_MS) } },
              },
            },
          },
        },
        investigate = { animation = "walk", motion = "moveToLastKnown", speed = STROLL },
        startle = { animation = "flinch", motion = "hold" },
        retreat = { animation = "run", motion = "moveToAnchor" },
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
