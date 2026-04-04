# Grounded Movement

> **Status:** pre-draft — depends on features not yet implemented. Depth and sub-tasks will be refined as dependencies land.
> **Blocked by:** Phase 1 (BSP loading), Phase 2 (input), Phase 3 (textured world) — see `context/plans/roadmap.md`
> **Related:** `context/lib/rendering_pipeline.md`, `context/lib/entity_model.md`, `context/lib/build_pipeline.md`

---

## Goal

Player controller with BSP collision, gravity, and ground detection. The player moves through a BSP level with physically grounded movement — no free-fly. Collisions against world geometry prevent wall clipping. Gravity pulls the player down; ground detection determines when the player is standing on a surface vs. falling.

---

## Scope

### In scope

- **BSP world collision** — player bounding volume tested against BSP geometry each movement step. The qbsp crate natively parses the `BRUSHLIST` BSPX lump, which provides convex brush hulls for collision detection. Use hull-based collision (point or bounding box traced against brush hulls) rather than per-triangle tests.
- **Gravity** — constant downward acceleration when not grounded. Terminal velocity cap.
- **Ground detection** — determine whether the player is standing on a walkable surface. Floor vs. wall vs. ceiling distinguished by surface normal angle threshold (e.g., surfaces within ~45 degrees of horizontal are walkable).
- **Slide movement** — when the player hits a wall, slide along it rather than stopping dead. Project remaining velocity onto the collision plane.
- **Step-up** — automatically step up small ledges (stair-stepping) without requiring a jump. Configurable step height.
- **Basic jump** — impulse velocity applied when grounded and jump action pressed.
- **Movement input** — forward/back/strafe from input action snapshot drives movement direction relative to player facing.

### Out of scope (follow-up tasks)

- Crouching, sprinting, swimming, ladders
- Entity-entity collision (player vs. enemies)
- Networked movement prediction
- Movement recording/replay

---

## Key decisions to make during refinement

- Collision hull dimensions (width, height, step height)
- Gravity and terminal velocity values
- Movement speed, acceleration, friction model (Quake-style air control vs. grounded-only acceleration)
- Whether to use a single trace (point + AABB) or clipnode-based hull selection like original Quake
- BRUSHLIST lump availability — verify that ericw-tools 2.0.0-alpha produces this lump for BSP2. If absent, fall back to clipnode-based collision from the standard BSP hull data.

---

## Acceptance criteria

- Player walks on flat surfaces and slopes within the walkable angle threshold.
- Player cannot walk through walls or fall through floors.
- Player slides along walls when moving into them at an angle.
- Player steps up small ledges automatically.
- Player falls when walking off an edge; gravity applies until landing.
- Jump launches the player upward; landing re-establishes grounded state.
