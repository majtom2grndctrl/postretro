--!strict
-- Reference data-context entity descriptors for the rotator and
-- damage-source reference behaviors, plus the map-placeable reference enemy.
-- Spread `M.referenceEntities` into the `ModManifest.entities` array to
-- register the archetypes.
--
-- See: context/lib/scripting.md §2

local M = {}

--- Classname for entities driven by `rotator_driver.{ts,luau}`.
M.ROTATOR_DRIVER_CLASSNAME = "game_rotator_driver"

--- Classname for entities targeted/observed by `damage_source.{ts,luau}`.
M.DAMAGE_SOURCE_CLASSNAME = "game_damage_source"

--- Classname for the map-placeable reference enemy (`health` + `mesh` + `ai`).
M.REFERENCE_ENEMY_CLASSNAME = "reference_enemy"

--- The map-placeable reference enemy: a full health + animated-mesh + AI-brain
--- archetype that exercises the M10 enemy loop end to end. It is directly
--- placeable from a `.map` via `"classname" "reference_enemy"` because it
--- carries `components.health` and `components.mesh` (the canonicalName
--- dispatch keys off those placeable components).
---
--- Model: the CC0 KayKit "Adventurers" Knight (Kay Lousberg), converted to the
--- engine's external-glTF layout under
--- `content/dev/models/reference_enemy_kaykit_knight/` and pruned to the four
--- clips named below. See that folder's `license.txt`.
---
--- The `mesh.animations` keys are author-defined logical state names; each maps
--- to one of the model's real clip names. The `ai.states` block maps the
--- brain's four logical states (idle / alert / attack / death) onto those mesh
--- state names -- the cross-component link the FSM drives each tick. (`alert`
--- is the brain's chase/pursuit state, which plays the `walk` locomotion clip.)
M.referenceEnemyEntity = defineEntity({
  canonicalName = M.REFERENCE_ENEMY_CLASSNAME,
  components = {
    -- Hit points + a hitscan hitbox so the shipped reference pistol's ray can
    -- target and kill it. ~1.8 m tall human silhouette; Y-up, so the middle
    -- half-extent is the vertical half-height and the offset lifts the box from
    -- the foot-level transform origin to mid-body.
    health = {
      max = 60,
      hitbox = {
        halfExtents = { 0.4, 0.9, 0.4 },
        offset = { 0, 0.9, 0 },
      },
      -- A headshot deals 2.5x, a leg shot 0.5x; unlisted zones apply 1.0.
      zoneMultipliers = {
        head = 2.5,
        leg = 0.5,
      },
    },
    -- Animated skinned mesh. The four state names are the engine-author vocab;
    -- each `clip` is one of the model's real (pruned) clip names.
    mesh = {
      model = "models/reference_enemy_kaykit_knight/scene.gltf",
      animations = {
        idle = { clip = "Idle", loop = true },
        walk = { clip = "Walking_A", loop = true, crossfadeMs = 200 },
        attack = {
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
    },
    -- The AI brain. Ranges are in metres; cooldown/despawn in ms; moveSpeed in
    -- m/s. `states` maps the brain's four logical states onto the mesh state
    -- names above.
    ai = {
      detectionRange = 16,
      attackRange = 2,
      leashRange = 24,
      attackDamage = 8,
      attackCooldownMs = 1200,
      moveSpeed = 3,
      deathDespawnMs = 4000,
      states = {
        idle = "idle",
        alert = "walk",
        attack = "attack",
        death = "death",
      },
    },
  },
})

--- Data-archetype entries used by the reference behaviors. The rotator and
--- damage-source entries are pure tag/transform carriers; the behaviors locate
--- their work via `worldQuery` filters on tags authored on the placement. The
--- reference enemy is a full health + mesh + ai archetype, map-placeable by its
--- `canonicalName`.
---
--- Spread into `ModManifest.entities`.
M.referenceEntities = {
  defineEntity({
    canonicalName = M.ROTATOR_DRIVER_CLASSNAME,
    components = { light = nil, emitter = nil },
  }),
  defineEntity({
    canonicalName = M.DAMAGE_SOURCE_CLASSNAME,
    components = { light = nil, emitter = nil },
  }),
  M.referenceEnemyEntity,
}

return M
