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
- Damage intent emitted as a typed hit event carrying hit position, normal, and an optional target entity id. Pre-emptive wiring — the consumer (health/kill) is the enemy-entity plan.
- Typed `fire` and `impact` sound events emitted on discharge and on a world hit. Pre-emptive wiring — the sink lands in the M10 stub-sound plan (next).

### Out of scope

- Projectile resolution mode — sibling mode, deferred (next weapon plan or M10 refinement). Do not fake it with a fast hitscan.
- Health component, damage application, kill path, ray-vs-entity targeting — all land in the **enemy entity + damage surface** plan, which extends the fire system to test entity volumes and consumes the hit event.
- Ammo and reload — `Action::Reload` stays unhandled. Weapon fires unlimited for now.
- Weapon switching, inventory, multiple equipped weapons.
- Material-aware impacts (per-surface spark/decal/sound). Requires a `cast_ray` extension to expose the hit triangle → `face_meta.material`; deferred.
- Spread / recoil / accuracy cone. Single ray down the crosshair.
- Muzzle flash, viewmodel.
- Audio backend and playback. The M10 stub-sound plan (next) and the M11 sound foundation own those; this plan only emits the `fire`/`impact` sound events.

## Acceptance criteria

- [ ] Pressing the fire action discharges the equipped weapon; firing again before its cooldown elapses does nothing.
- [ ] An auto-mode weapon fires repeatedly at its configured rate while the action is held; a semi-mode weapon fires once per press.
- [ ] A shot striking world geometry within range produces a visible impact effect at the hit surface, oriented to the surface normal.
- [ ] A shot into open space (no geometry within range) produces no impact and still consumes the cooldown.
- [ ] Weapon parameters are declared in a reference script via the SDK and drive runtime behavior; editing a value and hot-reloading (debug) changes firing behavior accordingly.
- [ ] `gen-script-types` output includes the weapon descriptor type and the type-definition drift test passes.
- [ ] The reference weapon script is present under `content/dev/scripts/` (or `sdk/`) and is the authoring template for a weapon.
- [ ] Discharging emits a `fire` sound event; a world hit emits an `impact` sound event (verifiable via the game-events log), though no audio backend consumes them yet.

## Tasks

### Task 1: Weapon data model
Add a weapon descriptor to the script surface and a runtime weapon component. Extend `EntityTypeDescriptor` with an optional weapon block (mirror the existing `movement` field). Add `ComponentKind::Weapon` and a weapon component implementing the `Component` trait, materialized from the descriptor at spawn (mirror `PlayerMovementComponent::from_descriptor`). Wire the SDK `defineEntity` weapon block, add the FGD weapon archetype, and regenerate type defs. Author the first weapon as a reference script (the authoring template) under `content/dev/scripts/`. The player equips one weapon at spawn via a weapon reference (canonical name) on the player descriptor — the natural grow path to weapon switching.

### Task 2: Aim ray
Promote the pitch-inclusive view-ray math (currently test-only in `camera.rs`) to a real `Camera` method returning ray origin (camera position) and normalized direction from yaw + pitch.

### Task 3: Hitscan fire system
A Rust system in the game-logic stage, after the movement tick (frame order: Input → Game logic). For the player's weapon component: read `Action::Shoot` per fire mode, decrement/check the cooldown timer against the fixed tick, and on a valid shot build the aim ray (Task 2), `cast_ray` against `CollisionWorld` clamped to weapon range, and produce the hit (point = `origin + dir*toi`, plus normal) or a miss. Reset the cooldown on fire. Emit the hit event on a hit, a `fire` sound event on every discharge, and an `impact` sound event on a world hit.

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
- **Impact** is the one genuinely new bit of infra (no one-shot emitter exists). Constraint: reuse the existing particle sim and billboard pass; the burst must self-clean without leaking entities. Exact lifetime/despawn mechanism is the implementer's call.
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

## Resolved decisions

1. **Hit event — in scope.** Emit a typed hit event (position, normal, optional target id) as pre-emptive wiring; the enemy-entity plan adds the health/kill consumer.
2. **Impact — build it.** Minimal self-cleaning particle burst; impacts are what make hitscan read.
3. **Equip — via descriptor reference.** Player descriptor names the default weapon by canonical name.
4. **Sound events — emit fire + impact.** The stub-sound plan ships second in M10 (right after this one), so the weapon emits typed `fire` and `impact` sound events; they are brief pre-emptive wiring until the sink lands one plan later. Event kinds are emitter-defined; the sink is generic over kind.
