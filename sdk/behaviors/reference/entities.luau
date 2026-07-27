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

--- Classname for the map-placeable reference enemy
--- (`health` + `mesh` + `behavior`).
M.REFERENCE_ENEMY_CLASSNAME = "reference_enemy"

--- Distance at which the reference enemy notices a player, in metres.
local DETECTION_RANGE = 16
--- Distance within which its melee swing connects, in metres.
local ATTACK_RANGE = 2
--- Legacy leash and this graph's acquisition radius, in metres.
local LEASH_RANGE = 50

--- The map-placeable reference enemy: a full health + animated-mesh +
--- behavior-graph archetype that exercises the M10 enemy loop end to end. It is
--- directly placeable from a `.map` via `"classname" "reference_enemy"` because
--- it carries `components.health` and `components.mesh` (the canonicalName
--- dispatch keys off those placeable components).
---
--- Model: the CC0 KayKit "Adventurers" Knight (Kay Lousberg), converted to the
--- engine's external-glTF layout under
--- `content/dev/models/reference_enemy_kaykit_knight/` and pruned to the four
--- clips named below. See that folder's `license.txt`.
---
--- The `mesh.animations` keys are author-defined names; each maps to one of the
--- model's real clip names. Every `behavior.states.*.animation` names one of
--- them -- the cross-component link the brain drives each tick.
---
--- The graph is the reference authoring of the classic three-state pursuit
--- shape:
---
--- ```text
---   ANY   --(!hasTarget || targetDied)-->               idle   (interrupt)
---   idle  --(acquisitionDue && dist <= ATTACK_RANGE)--> attack
---   idle  --(acquisitionDue && dist <= DETECTION_RANGE)--> alert
---   alert --(dist <= ATTACK_RANGE)-->                  attack
---   alert --(dist >  LEASH_RANGE)-->                   idle
---   attack --(dist >  ATTACK_RANGE)-->                 alert
--- ```
---
--- Three authoring notes worth copying:
---
--- * `acquisitionDue` conjunction. Detection is time-sliced by the engine's
---   think stride, so both `idle` edges only fire on an acquisition tick. The
---   IR has no `and` opcode yet, so the conjunction is spelled
---   `select(cond, inner, false)`. The attack-range and leash edges are
---   deliberately NOT gated: they must answer every tick, so a strided
---   acquisition gap can never suppress an in-range swing or hold a fled player
---   under pursuit.
--- * `idle -> attack` is declared first. Guards are first-true-wins in
---   declaration order, so the "already in contact range on the tick we notice
---   them" edge has to precede the plain detection edge to be reachable.
--- * The stand-down interrupts are declared first of all. Interrupts run before
---   any state-local guard, so losing the target outranks every range edge and
---   the enemy stands down in ONE tick. It is not optional polish:
---   `brain.targetDistance` reads a `1e9` sentinel with no target, and that
---   sentinel is ONE-DIRECTIONAL. `le`/`lt` guards read false untargeted (safe,
---   which is why the entry edges need no `hasTarget` conjunction), but
---   `gt`/`ge` guards read TRUE -- so without this interrupt the two disengage
---   guards below both fire on target loss and walk the enemy
---   `attack -> alert -> idle` over two ticks, playing the travel animation for
---   one of them with nothing to travel toward. The IR has no `not` opcode, so
---   the negation is spelled `select(hasTarget, false, true)`. The target facts
---   are meaningful only under `hasTarget`: they read zero (and `targetDied`
---   reads false) untargeted. Test death with the sweep's `targetDied` latch,
---   never `le(targetHealth, 0)`, which also fires with no target and misses
---   the latch's full definition.
---
--- There is no `death` state: death is not a graph transition. The engine's
--- death sweep latches a zero-HP enemy and the authored impact policy plays the
--- `death` mesh clip and despawns after its own delay -- despawn timing belongs
--- entirely to that policy's `despawn` effect, and the behavior block carries
--- no despawn field of its own.
---
--- The graph owns candidacy and disengagement separately. `candidateFilter`
--- runs once for every candidate the engine offers and only decides whether it
--- is eligible; it neither ranks candidates nor drops the target already held.
--- The state exits and interrupts above are the graph's disengagement policy.
--- There is no engine-side range limit on an authored `chaseTarget` state, so
--- the `alert` exit is still what stops pursuit.
---
--- `candidate.distance <= LEASH_RANGE` is this graph's acquisition radius. It
--- is deliberately 50, the legacy descriptor's `leashRange`, not its detection
--- range: the lowered legacy twin's floor applies that leash to acquisition and
--- retention from the enemy's current position. Keeping this filter at 50 makes
--- the authored and lowered traces agree.
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
      locomotion = { speedScale = true },
      animations = {
        idle = { clip = "Idle", loop = true },
        walk = { clip = "Walking_A", loop = true, crossfadeMs = 200, travelSpeed = 3 },
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
    -- The behavior state graph. Ranges are in metres; cooldown in ms; moveSpeed
    -- in m/s. Every `animation` names a `mesh.animations` key above.
    behavior = {
      initial = "idle",
      moveSpeed = 3,
      attack = { damage = 8, range = ATTACK_RANGE, cooldownMs = 1200 },
      -- Where engaged chasers STAND: the radius of the ring of combat slots the
      -- engine spreads them around the target. Pure spacing, and distinct from
      -- `attack.range` above, which gates DAMAGE and nothing else. Omitting it
      -- falls back to `attack.range`, which is the value used here -- it is
      -- authored explicitly anyway, because the two are separate knobs and a
      -- graph that later retunes its swing reach should not silently re-space
      -- its pack. A pure-pursuit graph (`chaseTarget`, no `action`) has no
      -- `attack.range` to fall back on and wants this field outright.
      engagementRadius = ATTACK_RANGE,
      -- Candidate eligibility is per graph and only narrows the engine's offer
      -- set. Dead pawns are never newly acquired; the distance clause is this
      -- graph's authored acquisition radius, not a descriptor range field.
      candidateFilter = runtime.select(
        candidate.died,
        false,
        runtime.le(candidate.distance, LEASH_RANGE)
      ),
      -- Stand down the instant the target is gone. Declared first so it
      -- outranks every state-local guard -- see the doc comment above for why
      -- the `gt` disengage guards below cannot do this job themselves.
      interrupts = {
        { to = "idle", when = runtime.select(brain.hasTarget, false, true) },
        -- The death sweep's latch is false untargeted. Do not compare
        -- `targetHealth` to zero: target facts are zero with no target too.
        { to = "idle", when = brain.targetDied },
      },
      states = {
        -- At rest. `initial` doubles as the state the engine forces when the
        -- aggro gate closes -- that gate is the only thing that overrides guard
        -- evaluation -- and as the animation a travelling state falls back to
        -- at a standstill, so it is authored rest-appropriate. Losing the
        -- target is NOT such an override: guards keep running, which is why the
        -- stand-down above is authored as an ordinary interrupt.
        idle = {
          animation = "idle",
          motion = "hold",
          transitions = {
            {
              to = "attack",
              when = runtime.select(
                brain.acquisitionDue,
                runtime.le(brain.targetDistance, ATTACK_RANGE),
                false
              ),
            },
            {
              to = "alert",
              when = runtime.select(
                brain.acquisitionDue,
                runtime.le(brain.targetDistance, DETECTION_RANGE),
                false
              ),
            },
          },
        },
        -- Pursuit. No action of its own, so the engine treats it as the
        -- locomotion state: it plays `walk` while travelling and yields to the
        -- `idle` rest animation when stopped.
        alert = {
          animation = "walk",
          motion = "chaseTarget",
          transitions = {
            { to = "attack", when = runtime.le(brain.targetDistance, ATTACK_RANGE) },
            { to = "idle", when = runtime.gt(brain.targetDistance, LEASH_RANGE) },
          },
        },
        -- Contact damage on the graph's `attack` cooldown while closing the
        -- last metre.
        attack = {
          animation = "attack",
          motion = "chaseTarget",
          action = "attack",
          transitions = {
            { to = "alert", when = runtime.gt(brain.targetDistance, ATTACK_RANGE) },
          },
        },
      },
    },
  },
})

--- Data-archetype entries used by the reference behaviors. The rotator and
--- damage-source entries are pure tag/transform carriers; the behaviors locate
--- their work via `worldQuery` filters on tags authored on the placement. The
--- reference enemy is a full health + mesh + behavior archetype, map-placeable
--- by its `canonicalName`.
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
