# Enemy Ranged / Hitscan Attacks — Design Intent

> **Status:** design intent / forward-looking — **NOT a ready spec.** Records how enemy ranged
> attacks are meant to grow on top of the shipped multi-attack `attacks` vocabulary, the
> load-bearing prerequisite they wait on, and the seams already placed for them. Feeds
> **Epic 16 › Resolution Modes** (`context/plans/roadmap.md`). Built demand-driven, with real
> consumers — never ahead of need.

## What ranged adds

Multi-attack (`E10--enemy-multi-attack`) ships a graph-wide named `attacks` map whose entries are
contact/melee: each carries `damage`, `maxRange`, `cooldownMs`, and an optional `engagementRadius`
inline, and fires by applying damage directly to the selected target within reach. A ranged attack
is a new **entry kind** in that same map: one that names a **weapon descriptor** instead of inline
contact stats.

This is the roadmap's *"attacks are weapons/wieldables"* model (`context/research/weapon-model.md`):
a `weapon`-referencing entry resolves its damage, reach, and cooldown from a canonical
`WeaponDescriptor`, so player and enemy attacks share one authoring substrate. The entry resolves
as **nearest-of hitscan**, reusing the player weapon ray path — `resolve_nearest_hit` /
`nearest_entity_hit` (`crates/postretro/src/weapon/mod.rs`,
`crates/postretro/src/scripting/systems/hit_zones.rs`) — rather than the direct-to-target contact
apply. Nearest-of means the ray damages the nearest entity it strikes, which need not be the
brain's selected target; world geometry occludes it (resolution-level occlusion, not a perception
model).

The `attacks` map is deliberately shaped to absorb this without a schema pivot: it is a name-keyed
map of entries from day one, so a `weapon` entry sits beside a contact entry, and reach-based
routing between them stays authored `@brain.targetDistance` transitions on the flat behavior graph.

## The load-bearing prerequisite: the player must become a hitscan target

Nearest-of hitscan cannot hit the player today, and this is the single fact that keeps enemy ranged
out of the melee-only spec.

The player pawn carries `components.health` but **deliberately no `hitbox`**
(`content/dev/scripts/player.ts`). A hitbox is what makes an entity hitscan-targetable
(`nearest_entity_hit`: *"Health without a hitbox is not targetable"*), so omitting it keeps the
player out of all weapon ray-targeting. Enemy damage reaches the player only through the direct
`apply_damage` chokepoint against the brain's **selected target** — the contact path multi-attack
ships. A nearest-of ray therefore sweeps past the player and lands on nothing, or on another enemy.

Enemy ranged combat requires reversing that documented invariant: the player becomes a
**first-class hitscan target** (it grows a hitbox, or an equivalent targetable presence). That is a
foundational combat-layer change, not an attack-vocabulary change, and it is why ranged defers to a
combat-layer spec rather than riding the multi-attack graph work.

## Consequences of a targetable player

Reversing the invariant is load-bearing in three places, each a reason the change is combat-layer
scope:

- **Self-hit foreclosure moves from data shape to an explicit parameter.** The player being
  hitbox-less silently forecloses self-hit on its own fire (the ray cannot resolve against a target
  that is not targetable). Once the player is targetable, that foreclosure must become an explicit
  **shooter-exclusion parameter** on `nearest_entity_hit`, passed by every caller — the enemy fire
  path *and* the player's own fire path, which now needs it too.
- **Co-op hit authority.** Enemy ranged hits against a player fold into the client-authoritative
  hit-declaration path (`HitDeclaration` / `PendingHitDeclarations` /
  `host_flush_pending_hit_declarations`, `crates/postretro/src/netcode/mod.rs`): who is authoritative
  over an enemy-to-player hit, and how it reconciles, is per-interaction authority work the way each
  Epic 16 resolution mode carries its own.
- **Friendly fire becomes reachable.** A nearest-of ray can put a bystanding enemy in the line of
  fire, making enemy-on-enemy impacts possible. Whether such an impact deals damage is the
  **Faction & relationship model**'s per-pair policy (`roadmap.md`, Epic 10), not an engine floor
  decision — see `context/research/enemy-aggro-model.md`.

## The two-home question for resolved weapon stats

A `weapon` entry's effective damage, reach, and cooldown are unknown until the referenced descriptor
resolves at spawn (a `find_descriptor` scan over the entity-type descriptors, the lookup
`entity_class` and a player's default-weapon name already use). Two homes for the resolved stats:

- **Bake into the retained graph at spawn** — resolve once, write the effective numbers back into
  the brain's retained `attacks` map, so per-tick reads stay descriptor-local (the melee-only spec's
  posture: every stat inline, no side table).
- **A threaded spawn-time tuning table** — a name-indexed derived table alongside the brain,
  rebuilt from the graph whenever the entity is seen (the bound-guard-program precedent,
  `entity_model.md` §7c).

The melee-only spec needs neither, because contact stats are inline and known at parse. Ranged
forces the choice, since weapon range gates both the fire reach and the per-attack standoff and is
only known after resolution. The firing-origin question rides here too: a hitscan ray needs an
origin, and a visible ranged shot (beam, muzzle flash, traveling projectile) needs a posed
weapon-socket origin rather than the hitbox center a bare occlusion/self-exclusion ray can use.

## AI prerequisites surfaced by the limitator experiment

An experimental ranged enemy (`content/dev/scripts/limitator.ts` — a rifleman authored on the
shipped melee `attacks` / behavior-graph substrate) exercised the AI floor ahead of this spec and
surfaced two gaps a functional ranged enemy needs closed. Both are AI-layer (Epic 10 lineage), but
each bites only a hold-at-standoff enemy, so they are recorded here rather than re-derived later.

- **Combat positioning must seat a ranged enemy inside its own fire threshold.** E10 combat
  positioning (`crates/postretro/src/combat_positioning.rs`) scores slots by
  `|slot_to_target − engagement_radius|`, so the preferred slot sits at *exactly* `engagement_radius`
  from the target and grounding preserves that XZ distance. An enemy whose fire guard is
  `targetDistance ≤ engagement_radius` therefore settles just *outside* the guard (the steering
  hard-stop lands it at `engagement_radius` plus a fraction of the agent radius) and never crosses
  into firing range — it stands at the ring playing its locomotion clip until the target closes the
  gap. A melee enemy hides this because its contact guard sits well inside its slot ring. Ranged
  standoff needs positioning that targets a firing distance strictly *inside* the fire threshold — a
  standoff band, or hysteresis on the guard — not the ring radius itself. Content can work around it
  today (author `engagementRadius` below the guard distance), but a first-class ranged enemy should
  not require the author to reverse-engineer the slot-scoring interaction.
- **Facing must track the target through a committed aim, and the shot must gate on facing.** A
  committed aim activity (hold motion, no steering `engages_path` contribution) freezes the enemy's
  facing for the aim duration, and the fire tick applies damage regardless of where the muzzle
  points — so a ranged enemy can deal damage while visibly aimed away from its target during the yaw
  slew. Melee windups hide this (short, and contact is omnidirectional); a long rifle aim does not. A
  ranged attack wants the aim phase to keep slewing toward the target and the fire resolution to gate
  on — or originate from — the posed weapon socket's forward direction.

## Ownership

Enemy ranged / hitscan attacks — the `weapon`-referencing entry kind, nearest-of ray resolution,
the targetable-player prerequisite, its self-exclusion and co-op consequences, and the resolved-stat
home — belong to a future **combat-layer spec** under **Epic 16 › Resolution Modes**
(`context/plans/roadmap.md`), which owns combat interaction authority and the wieldable/weapon
substrate. This note fixes the design so that spec does not re-derive it.
