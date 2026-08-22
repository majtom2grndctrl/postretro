--!strict
-- Reference data-context entity descriptors for the rotator and damage-source
-- behaviors plus the SDK-owned E21 pose fixture. The dev-only reference enemy
-- lives in `content/dev/scripts/reference-enemy.luau`.

local M = {}

M.ROTATOR_DRIVER_CLASSNAME = "game_rotator_driver"
M.DAMAGE_SOURCE_CLASSNAME = "game_damage_source"
M.POSE_FIXTURE_ENEMY_CLASSNAME = "pose_fixture_enemy"

local DETECTION_RANGE = 16
local JAB_RANGE = 2
local POSE_FIXTURE_AGGRO_RANGE = 50

M.poseFixtureEnemyEntity = defineEntity({
  canonicalName = M.POSE_FIXTURE_ENEMY_CLASSNAME,
  components = {
    health = {
      max = 60,
      hitbox = {
        halfExtents = { 0.4, 0.9, 0.4 },
        offset = { 0, 0.9, 0 },
      },
      zoneMultipliers = { head = 2.5 },
    },
    mesh = {
      model = "models/pose-modifier-fixture/joint_zones.gltf",
      attachments = { hand_r = "models/attachment-marker/hand-prop.gltf" },
      animations = {
        idle = { clip = "Rest", loop = true },
        walk = { clip = "Rest", loop = true },
        attack = { clip = "Rest", loop = true },
        death = { clip = "Rest", loop = true },
      },
      defaultState = "idle",
    },
    behavior = {
      initial = "idle",
      moveSpeed = 3,
      attacks = {
        jab = { damage = 8, maxRange = JAB_RANGE, cooldownMs = 1200 },
      },
      engagementRadius = JAB_RANGE,
      candidateFilter = candidate.died["not"](candidate.died)["and"](
        candidate.died["not"](candidate.died),
        candidate.distance:le(POSE_FIXTURE_AGGRO_RANGE)
      ),
      activities = {
        idle = { animation = "idle", motion = "hold" },
        alert = { animation = "walk", motion = "chaseTarget" },
        attack = {
          animation = "attack",
          motion = "chaseTarget",
          action = { attack = "jab" },
        },
      },
      transitions = {
        ["*"] = {
          { to = "idle", when = brain.hasTarget["not"](brain.hasTarget) },
          { to = "idle", when = brain.targetDied },
          { to = "idle", when = brain.targetDistance:gt(POSE_FIXTURE_AGGRO_RANGE) },
        },
        idle = {
          {
            to = "attack",
            when = brain.acquisitionDue["and"](
              brain.acquisitionDue,
              brain.targetDistance:le(JAB_RANGE)
            ),
          },
          {
            to = "alert",
            when = brain.acquisitionDue["and"](
              brain.acquisitionDue,
              brain.targetDistance:le(DETECTION_RANGE)
            ),
          },
        },
        alert = {
          { to = "attack", when = brain.targetDistance:le(JAB_RANGE) },
          { to = "idle", when = brain.targetDistance:gt(POSE_FIXTURE_AGGRO_RANGE) },
        },
        attack = { { to = "alert", when = brain.targetDistance:gt(JAB_RANGE) } },
      },
    },
  },
})

M.referenceEntities = {
  defineEntity({
    canonicalName = M.ROTATOR_DRIVER_CLASSNAME,
    components = { light = nil, emitter = nil },
  }),
  defineEntity({
    canonicalName = M.DAMAGE_SOURCE_CLASSNAME,
    components = { light = nil, emitter = nil },
  }),
  M.poseFixtureEnemyEntity,
}

return M
