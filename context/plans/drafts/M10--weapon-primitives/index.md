# Weapon Primitives

## Goal

Establish the engine's weapon primitive surface: a script-declared weapon archetype plus a Rust fire system that hitscans against the static world and spawns an impact. First weapon authored in the SDK as a reference behavior. A foundation to refine — hitscan only, one equipped weapon, projectiles and entity damage deferred. It must read as game-y (you point, click, see a hit), not be a stub.

## Model anchor

Weapons follow the M7 movement precedent exactly: **script declares parameters, Rust runs the logic.** The VM is not live during gameplay. A weapon descriptor (damage, range, fire rate, fire mode) materializes into a per-player weapon component; a Rust system reads `Action::Shoot` each game-logic tick and runs the fire logic. See `research.md` for the confirmed `PlayerMovementComponent` parallel.

## Scope

### In scope

- Weapon archetype declared via the SDK (`defineEntity` weapon block) → FGD entry → runtime component. Parameters: damage, range, fire-rate (cooldown), fire mode (semi/auto), resolution mode (hitscan).
- Per-player weapon state: equipped weapon params + cooldown timer. Player equips one weapon at spawn.
- Rust fire system in the game-logic stage: reads `Action::Shoot` (edge for semi, held for auto), gates on cooldown, builds the aim ray, casts against `CollisionWorld`, computes the world hit.
- Aim ray: a `Camera` view-ray method (origin + pitch-inclusive direction).
- Impact effect: a transient particle burst at the world hit point, oriented by the surface normal, that cleans itself up.
- Damage intent emitted as a typed hit event carrying hit position, normal, and an optional target entity id (pre-emptive wiring — the consumer is the enemy-entity plan). See open questions.

### Out of scope

- Projectile resolution mode — sibling mode, deferred (next weapon plan or M10 refinement). Do not fake it with a fast hitscan.
- Health component, damage application, kill path, ray-vs-entity targeting — all land in the **enemy entity + damage surface** plan, which extends the fire system to test entity volumes and consumes the hit event.
- Ammo and reload — `Action::Reload` stays unhandled. Weapon fires unlimited for now.
- Weapon switching, inventory, multiple equipped weapons.
- Material-aware impacts (per-surface spark/decal/sound). Requires a `cast_ray` extension to expose the hit triangle → `face_meta.material`; deferred.
- Spread / recoil / accuracy cone. Single ray down the crosshair.
- Muzzle flash, viewmodel.
- Real audio. Fire/impact sound events may be emitted as wiring (see open questions); playback is the M11 sound plan.

## Acceptance criteria

- [ ] Pressing the fire action discharges the equipped weapon; firing again before its cooldown elapses does nothing.
- [ ] An auto-mode weapon fires repeatedly at its configured rate while the action is held; a semi-mode weapon fires once per press.
- [ ] A shot striking world geometry within range produces a visible impact effect at the hit surface, oriented to the surface normal.
- [ ] A shot into open space (no geometry within range) produces no impact and still consumes the cooldown.
- [ ] Weapon parameters are declared in a reference script via the SDK and drive runtime behavior; editing a value and hot-reloading (debug) changes firing behavior accordingly.
- [ ] `gen-script-types` output includes the weapon descriptor type and the type-definition drift test passes.
- [ ] The reference weapon script is present under `content/dev/scripts/` (or `sdk/`) and is the authoring template for a weapon.

## Tasks

### Task 1: Weapon data model
Add a weapon descriptor to the script surface and a runtime weapon component. Extend `EntityTypeDescriptor` with an optional weapon block (mirror the existing `movement` field). Add `ComponentKind::Weapon` and a weapon component implementing the `Component` trait, materialized from the descriptor at spawn (mirror `PlayerMovementComponent::from_descriptor`). Wire the SDK `defineEntity` weapon block, add the FGD weapon archetype, and regenerate type defs. Author the first weapon as a reference script (the authoring template) under `content/dev/scripts/`. The player equips one weapon at spawn — decide equip source in open questions.

### Task 2: Aim ray
Promote the pitch-inclusive view-ray math (currently test-only in `camera.rs`) to a real `Camera` method returning ray origin (camera position) and normalized direction from yaw + pitch.

### Task 3: Hitscan fire system
A Rust system in the game-logic stage, after the movement tick (frame order: Input → Game logic). For the player's weapon component: read `Action::Shoot` per fire mode, decrement/check the cooldown timer against the fixed tick, and on a valid shot build the aim ray (Task 2), `cast_ray` against `CollisionWorld` clamped to weapon range, and produce the hit (point = `origin + dir*toi`, plus normal) or a miss. Reset the cooldown on fire. Emit the hit event if that lands in scope.

### Task 4: Impact effect
A transient impact spawned at the Task 3 hit point: a short particle burst reusing the billboard/particle pipeline, oriented by the surface normal, that despawns after its burst without leaking entities. The fire system calls the impact spawner on a world hit.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — independent (data model vs. camera math).
**Phase 2 (sequential):** Task 3 — consumes the weapon component (Task 1) and the aim ray (Task 2).
**Phase 3 (sequential):** Task 4 — consumes the hit point from Task 3.

## Rough sketch

- **Descriptor → component** mirrors movement: validated descriptor fields, `from_descriptor` copies into live component state at spawn. Keep the weapon component on the player entity (a `Weapon` component alongside `PlayerMovement`).
- **Fire mode** is a descriptor enum; the fire system maps semi → `ButtonState::Pressed`, auto → `is_active()`.
- **Cooldown** counts down in fixed-tick units; fire rate is expressed as a period (ms or ticks) in the descriptor, converted at materialization.
- **Range** clamps `max_toi` on the cast.
- **Impact** is the one genuinely new bit of infra (no one-shot emitter exists). Constraint: reuse the existing particle sim and billboard pass; the burst must self-clean. Exact lifetime/despawn mechanism is the implementer's call — see open questions for the build-vs-defer decision.
- Named identifiers and current signatures are inventoried in `research.md`; confirm against source before editing.

## Boundary inventory

Casing for the weapon archetype across boundaries. Field names are camelCase on the script/wire surface (`#[serde(rename_all = "camelCase")]` convention), snake_case in Rust.

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Weapon component kind | `ComponentKind::Weapon` | n/a | n/a | n/a | n/a |
| descriptor block | `weapon: Option<WeaponDescriptor>` | `"weapon"` | `weapon` | `weapon` | n/a |
| damage | `damage: f32` | `"damage"` | `damage` | `damage` | `damage(float)` |
| range | `range: f32` | `"range"` | `range` | `range` | `range(float)` |
| fire rate | `cooldown_ms` (or ticks) | `"fireRateMs"` | `fireRateMs` | `fireRateMs` | `fire_rate_ms(float)` |
| fire mode | `fire_mode: FireMode` | `"fireMode"` | `fireMode` | `fireMode` | `fire_mode(choices)` |
| resolution mode | `resolution: ResolutionMode` | `"resolution"` | `resolution` | `resolution` | n/a (hitscan only) |

(Concrete field names above are proposed, not load-bearing — pin them during implementation. The casing rule is the durable part.)

## Open questions

1. **Hit event in scope?** Recommend yes: emit a typed hit event (position, normal, optional target id) now as pre-emptive wiring, so the enemy-entity plan drops in the health/kill consumer without touching the fire path. Alternative: defer the event entirely to the enemy plan, leaving the weapon plan world-only. Decision affects whether Task 3 has an event output.
2. **Impact: build or defer?** A one-shot burst emitter doesn't exist. Recommend building a minimal self-cleaning burst (impacts are what make hitscan read). Alternative for maximum lean: defer all visual impact, emit only an impact event + log for v1. Decision sizes Task 4.
3. **Equip source.** Does the player descriptor name a default weapon by canonical name, or does v1 hardcode equipping the single declared weapon at player spawn? Recommend the descriptor reference — small, and it's the natural grow path to weapon switching.
4. **Fire/impact sound events.** Emit now as wiring for the M11 sound plan, or leave the weapon silent until then? Cheap to emit; recommend emitting.
