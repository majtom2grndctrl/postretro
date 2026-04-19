# Entity Types

> **Status:** future brainstorm — blocked until post-Milestone 11 (scripting layer). Concrete entity types (door, enemy, pickup, etc.) are implemented as scripts, not Rust engine types. Do not refine or implement until the scripting runtime and entity API bindings from M11 exist.
> **Blocked by:** Milestone 7 (entity model foundation), Milestone 11 (scripting layer), Milestone 12 (player movement), Milestone 13 (weapons), Milestone 14 (NPCs) — see `context/plans/roadmap.md`.
> **Related:** `context/lib/entity_model.md`, `context/lib/rendering_pipeline.md`, `context/lib/audio.md`, `context/plans/drafts/entity-model-foundation/index.md`

---

## Goal

Define and implement specific entity types within the entity model framework. Each type is a concrete implementation with type-specific data and behavior — not a generic component composition.

---

## Entity types to define

| Type | Source | Visual | Behavior |
|------|--------|--------|----------|
| **Player** | Runtime (single instance) | First-person (weapon sprite) | Input-driven movement, health, weapon state |
| **Enemy** | BSP entity lump | Billboard sprite | AI state machine, patrol/chase/attack, health, drops |
| **Door** | BSP entity lump | Brush model (BSP submodel) | Triggered open/close, blocks movement when closed |
| **Pickup** | BSP entity lump | Billboard sprite (rotating/bobbing) | Collected on player touch, grants item/ammo/health |
| **Trigger** | BSP entity lump | Invisible brush volume | Fires event when player enters (door open, trap, scripted event) |
| **Projectile** | Runtime (spawned by weapon/enemy) | Billboard sprite or point light | Linear or arc movement, BSP collision, damage on hit |

---

## Scope

### In scope

- Concrete type definitions with type-specific data
- BSP entity lump parsing: classname → engine entity type resolution
- Per-type update logic within the fixed-timestep game loop
- Game event emission (damage, sound triggers, state changes)
- Entity-entity interaction (projectile hits enemy, player touches pickup)

### Out of scope (follow-up tasks)

- Specific enemy AI behaviors (patrol paths, sight checks, attack patterns)
- Weapon definitions and balance
- Level scripting beyond simple triggers
- Boss entities
- Particle effect entities

---

## Key decisions to make during refinement

- Enemy AI model: state machine states and transitions
- Door mechanics: key-locked, timed, toggle
- Pickup types and their effects
- Trigger types: once vs. repeatable, delay
- Projectile types: hitscan vs. physical, splash damage radius
- Entity spawn/despawn rules (do pickups respawn?)

---

## Acceptance criteria

- Each entity type instantiates from BSP entity lump data (or runtime spawning for projectiles).
- Entities update each tick with type-appropriate behavior.
- Entity-entity interactions produce correct game events.
- Unknown classnames in BSP entity lump log a warning and are skipped.
