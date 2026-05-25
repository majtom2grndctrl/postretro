# Weapon Primitives

## Goal

Establish the engine's weapon primitive surface: a script-declared weapon archetype plus a Rust fire system that hitscans against the static world and spawns an impact. First weapon authored in the SDK as a reference behavior. A foundation to refine — hitscan only, one equipped weapon, projectiles and entity damage deferred. It must read as game-y (you point, click, see a hit), not be a stub.

## Model anchor

Weapons follow the M7 movement precedent exactly: **script declares parameters, Rust runs the logic.** The VM is not live during gameplay. A weapon descriptor (damage, range, fire rate, fire mode) materializes into a `Weapon` component on a separate wieldable **instance entity**; the player holds an active-wieldable reference (an `EntityId`) to it. A Rust system reads `Action::Shoot` each game-logic tick and runs the fire logic. See `research.md` for the confirmed `PlayerMovementComponent` parallel, and `context/research/weapon-model.md` §7 (load-bearing invariants) / §8 (M10 mapping) for the long-term shape this slice honors.

## Scope

### In scope

- Weapon archetype declared via the SDK (`defineEntity` weapon block) → FGD entry → runtime component. Parameters: damage, range, fire-rate (cooldown), fire mode (semi/auto), resolution mode (hitscan).
- Per-instance weapon state, referenced by the player: weapon params + cooldown timer live on a wieldable instance entity; the player holds an active-wieldable reference (`EntityId`) to it. Player equips one weapon at spawn.
- Rust fire system in the game-logic stage: reads `Action::Shoot` (edge for semi, held for auto), gates on cooldown, builds the aim ray, casts against `CollisionWorld`, computes the world hit.
- Aim ray: a `Camera` view-ray method (origin + pitch-inclusive direction).
- Impact effect: a transient particle burst at the world hit point, oriented by the surface normal, that cleans itself up.
- Damage intent resolved as an `ActivationOutcome::Hit(DamagePayload)` — a struct payload (amount only for M10), never a bare scalar. The spatial impact info (hit point, normal, optional target id) rides separately, not in the payload. Pre-emptive wiring — the consumer (health/kill) is the enemy-entity plan.
- Typed `activate` and `impact` sound events emitted on discharge and on a world hit — the weapon's primary activation. Pre-emptive wiring — the sink lands in the M10 stub-sound plan (next).

### Out of scope

- Projectile resolution mode — sibling mode, deferred (next weapon plan or M10 refinement). Do not fake it with a fast hitscan.
- Health component, damage application, kill path, ray-vs-entity targeting — all land in the **enemy entity + damage surface** plan, which extends the fire system to test entity volumes and consumes the damage payload.
- Ammo and reload — `Action::Reload` stays unhandled. Weapon fires unlimited for now.
- Weapon switching, inventory, multiple equipped weapons.
- Material-aware impacts (per-surface spark/decal/sound). Requires a `cast_ray` extension to expose the hit triangle → `face_meta.material`; deferred.
- Spread / recoil / accuracy cone. Single ray down the crosshair.
- Muzzle flash, viewmodel.
- Audio backend and playback. The M10 stub-sound plan (next) and the M11 sound foundation own those; this plan only emits the `activate`/`impact` sound events.

## Acceptance criteria

- [ ] Pressing the fire action discharges the equipped weapon; firing again before its cooldown elapses does nothing.
- [ ] An auto-mode weapon fires repeatedly at its configured rate while the action is held; a semi-mode weapon fires once per press.
- [ ] A shot striking world geometry within range produces a visible impact effect at the hit surface, oriented to the surface normal.
- [ ] A shot into open space (no geometry within range) produces no impact and still consumes the cooldown.
- [ ] Weapon parameters are declared in a reference script via the SDK and drive runtime behavior; editing a value and hot-reloading (debug) changes firing behavior accordingly.
- [ ] `gen-script-types` output includes the weapon descriptor type and the type-definition drift test passes.
- [ ] The reference weapon script is present under `content/dev/scripts/` (or `sdk/`) and is the authoring template for a weapon.
- [ ] Discharging emits an `activate` sound event; a world hit emits an `impact` sound event. Verified by a unit test asserting on the event names the inner fire tick returns via a `WeaponFireEvents` typed struct (analogous to `MovementEvents`), before the caller maps to `Vec<&'static str>` (no audio backend or event log consumes them yet).

## Tasks

### Task 1: Weapon data model
Add a weapon descriptor to the script surface and a runtime weapon component. Extend `EntityTypeDescriptor` with an optional weapon block (mirror the existing `movement` field). Add `ComponentKind::Weapon` and a weapon component implementing the `Component` trait, materialized from the descriptor at spawn (mirror `PlayerMovementComponent::from_descriptor`). Append `Weapon` to the `VARIANTS` array in `registry.rs` and assign it the next discriminant (`7`); `COUNT` derives from that array and backs the component-storage array index. The component lives on a separate wieldable instance entity, not the player. Wire the SDK `defineEntity` weapon block and regenerate type defs. The weapon block is authored as `components: { weapon: {...} }` in JS/Luau (mirroring how `movement` is authored); the hand-written JS/Luau parser reads `components.weapon`, not a top-level `weapon` key. Extend the typedef generator's type-name mapping to emit `WeaponDescriptor`, `FireMode`, and `ResolutionMode`, and add `weapon?` to the `EntityTypeComponents` typedef bag — new Rust types do not auto-appear in the generator. **No FGD weapon archetype:** weapon params are descriptor-only (not map-overridable, see Boundary inventory), and in M10 the weapon is not map-placed — it is spawned as a companion at player spawn and referenced by canonical name. (Open: a pickup/placeable weapon — a future slice — would need *some* FGD presence to be placed in a map, but not exposed params. Out of scope here.) Author the first weapon as a reference script (the authoring template) under `content/dev/scripts/`. Add `default_weapon: Option<String>` to `EntityTypeDescriptor` (decision 7). At spawn, after creating the player entity, resolve the canonical name through the entity type registry, spawn the weapon entity via the normal data-archetype path, and store the returned `EntityId` in `App::active_wieldable: Option<EntityId>` (decision 8, 9). The `String` canonical name lives only in the descriptor; the runtime field is the resolved `EntityId`. `active_wieldable` is the chokepoint a future inventory plan replaces without touching the fire system.

### Task 2: Aim ray
Promote the pitch-inclusive view-ray math (currently test-only in `camera.rs`) to a real `Camera` method returning ray origin (camera position) and normalized direction from yaw + pitch. Specifically, promote the `look_dir` `Vec3` expression from within the `#[cfg(test)]` `view_matrix()` function — do not promote the `Mat4::look_at_rh` machinery; only the normalized direction vector is needed.

### Task 3: Hitscan fire system
A Rust system in the game-logic stage, after the movement tick (frame order: Input → Game logic). The fire system lives in `crates/postretro/src/weapon/mod.rs` within the `postretro` crate, keeping `pub(crate)` APIs (`cast_ray`, `EntityId`) reachable. Resolve the weapon component off the player's active-wieldable reference. Read `Action::Shoot` per fire mode, decrement/check the cooldown timer against the fixed tick, and on a valid shot build the aim ray (Task 2), `cast_ray` against `CollisionWorld` clamped to weapon range. The tick reads damage/range/cooldown through a `WeaponComponent::effective() -> EffectiveStats` accessor, never raw component fields (vision invariant §7 #2); for M10 the accessor is identity passthrough — base stats, no rolls/augments, no caching — and returns named fields (not a `StatId → value` map, a deferred §9 fork). The tick returns an `ActivationOutcome`: for M10 only `Hit(DamagePayload)` is populated (the spatial impact info — point = `origin + dir*toi`, normal — is carried separately, not in the payload); `Effect`/`Spawned` are declared-but-unreachable variants holding the seam open (§7 #8). Reset the cooldown on fire. Emit an `activate` sound event on every discharge and an `impact` sound event on a world hit. There are two channels: sound events drain to the reaction system as strings; the damage outcome is consumed in-process by the (future) health system — the payload never rides the string channel.

### Task 4: Impact effect
A transient impact spawned at the Task 3 hit point: a short, visually readable particle burst reusing the billboard/particle pipeline, oriented by the surface normal, that despawns after its burst without leaking entities. The burst's look is hardcoded for M10. The fire system triggers it through **one named impact-effect chokepoint** — "spawn impact effect at (point, normal)" — never by inlining particle spawning in the fire tick. This is the effect analog of the `effective()` / `ActivationOutcome` seams: M10's chokepoint spawns the one hardcoded burst, and a future data-defined effect descriptor (per-weapon / per-material, deferred with material-aware impacts) resolves behind the same chokepoint without touching the fire system. Hardcoded-but-seamed, so the burst becomes the default effect later rather than throwaway code.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — independent (data model vs. camera math).
**Phase 2 (sequential):** Task 3 — consumes the weapon component (Task 1) and the aim ray (Task 2).
**Phase 3 (sequential):** Task 4 — consumes the hit point from Task 3.

## Rough sketch

- **Descriptor → component** mirrors movement: validated descriptor fields, `from_descriptor` copies into live component state at spawn. The `Weapon` component lives on a separate wieldable instance entity; `App::active_wieldable: Option<EntityId>` points at it (decision 9). This is invariant #1 of the wieldables vision (`context/research/weapon-model.md` §7) — per-instance state is what later lets two instances carry different rolls and lets switch/pickup preserve per-instance cooldown/ammo. A player-fields model collapses the moment a second instance with different stats exists.
- **Effective-stats seam.** The fire tick reads stats through `WeaponComponent::effective() -> EffectiveStats`, not raw fields (§7 #2). M10 is identity passthrough — base stats, no rolls/augments, no caching — but the seam exists so rolls/augments slot in later without touching the fire tick. The accessor returns named fields for M10; the `StatId → value` map is a deferred §9 fork.
- **Activation outcome seam.** The fire tick returns `enum ActivationOutcome { Hit(DamagePayload), Effect(..), Spawned(EntityId) }` (§7 #8). Only `Hit` is built. `DamagePayload { amount: f32 }` is a struct from day one (§7 #3 — a payload is a struct, never a bare scalar) so damage types/crit/falloff grow inside it without re-threading consumers. The variant name `Hit` is the decision-of-record here; `weapon-model.md` §7 #8 uses `Damage` inconsistently — ignore that; §8 uses `Hit` and this spec follows §8. The spatial impact info (point, normal) for spawning the effect rides separately, not in the payload.
- **Two-channel return** mirrors the real codebase pattern `movement::tick` → `run_movement_tick` (a typed `MovementEvents` struct inner, `Vec<&'static str>` at the caller). The inner fire tick returns a typed outcome plus emitted events; the caller maps the event strings into the reaction system and consumes the damage outcome in-process. Sound events go through the string channel; the `DamagePayload` does not.
- **Fire mode** is a descriptor enum; the fire system maps semi → `ButtonState::Pressed`, auto → `is_active()`.
- **Cooldown** counts down in fixed-tick units; fire rate is expressed as a period (ms or ticks) in the descriptor, converted at materialization.
- **Range** clamps `max_toi` on the cast.
- **Impact** is the one genuinely new bit of infra (no one-shot emitter exists). Constraint: reuse the existing particle sim and billboard pass; the burst must self-clean without leaking entities. Exact lifetime/despawn mechanism is the implementer's call. Invoke it through a single impact-effect chokepoint (see Task 4) so a data-defined effect descriptor can replace the hardcoded burst behind the seam later.
- Named identifiers and current signatures are inventoried in `research.md`; confirm against source before editing.

## Boundary inventory

Casing for the weapon archetype across boundaries. Field names are camelCase on the script/wire surface (`#[serde(rename_all = "camelCase")]` convention), snake_case in Rust.

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| Weapon component kind | `ComponentKind::Weapon` | n/a | n/a | n/a |
| default weapon | `default_weapon: Option<String>` | `"defaultWeapon"` | `defaultWeapon` | `defaultWeapon` |
| descriptor block | `weapon: Option<WeaponDescriptor>` | `"weapon"` | `components.weapon` | `components.weapon` |
| damage | `damage: f32` | `"damage"` | `damage` | `damage` |
| range | `range: f32` | `"range"` | `range` | `range` |
| fire rate | `cooldown_ms` (or ticks) | `"fireRateMs"` | `fireRateMs` | `fireRateMs` |
| fire mode | `fire_mode: FireMode` | `"fireMode"` | `fireMode` | `fireMode` |
| resolution mode | `resolution: ResolutionMode` | `"resolution"` | `resolution` | `resolution` |

(Concrete field names above are proposed, not load-bearing — pin them during implementation. The casing rule is the durable part.)

**Weapon params are descriptor-only — not FGD-exposed**, matching the movement precedent (movement params are passed verbatim from the descriptor, not overridable from the map). A map must not be able to retune a weapon; a *future* scripted reaction primitive may mutate params at runtime (e.g. an event that changes fire rate) — this is the justification for not using FGD KVPs, not a capability available in M10. This is why there is no FGD KVP column.

## Resolved decisions

1. **Damage outcome — in scope.** The fire tick resolves into `ActivationOutcome::Hit(DamagePayload)` — a struct payload, never a bare scalar (§7 #3), behind a named outcome seam (§7 #8). The enemy-entity plan adds the health/kill consumer. The spatial impact info (point, normal) used to spawn the effect rides separately, not in the payload.
2. **Impact — build it.** Minimal self-cleaning particle burst; impacts are what make hitscan read.
3. **Equip — via descriptor reference.** Player descriptor names the default weapon by canonical name.
4. **Archetype declaration — a `weapon` block on `defineEntity`.** M10 commits to a `weapon` block on the entity definition (mirroring `movement`), not a standalone `defineWeapon`. A known, accepted cost — the §9 declaration-surface fork resolves the other way for the looter shape, but the movement parallel keeps M10 cheap and consistent.
5. **Active reference — a bare `EntityId`.** The player stores the active wieldable as a bare `EntityId` and the `Weapon` component is resolved off it. This defers the wieldable marker-vs-kind-tag typing choice (§9): the player field keys on "wieldable instance," so a second kind drops in without rework.
6. **Sound events — emit activate + impact.** The stub-sound plan ships second in M10 (right after this one), so the weapon emits typed `activate` and `impact` sound events; they are brief pre-emptive wiring until the sink lands one plan later. Event kinds are emitter-defined; the sink is generic over kind. Conceptually these belong to the weapon's **primary activation**; M10 keeps the descriptor flat; the future `primary: { use: "fire", emits: { activate, impact } }` shape (vision §8) is noted in a doc comment on the descriptor struct — no runtime field in M10. No `secondary` block and no `use` discriminator now — those are correctly deferred §8 seams.
7. **Default weapon field — `default_weapon: Option<String>` on `EntityTypeDescriptor`.** The player archetype names its starting weapon by canonical name via a new top-level `defaultWeapon?` field on `EntityTypeDescriptor` (alongside `movement`, `light`, etc.) — authored as `defineEntity({ movement: {...}, defaultWeapon: "pistol" })`. It is not nested under `movement`; equip is a different concern at the same level. Not an FGD KVP.
8. **Spawn wiring.** After `spawn_from_player_starts` creates the player entity, the caller checks `descriptor.default_weapon`, resolves it through the entity type registry by canonical name, spawns the weapon entity (via the normal data-archetype path), and stores the returned `EntityId` in the app-level active-wieldable field (decision 9). The weapon entity is a sibling, not a child — no parent/child linkage needed.
9. **Active-wieldable runtime storage — app-level field.** `active_wieldable: Option<EntityId>` lives on the app struct (not on a component, not on `PlayerMovementComponent`). There is only one player; networking is a non-goal; a per-component field would require a new component kind with its associated `VARIANTS`/`COUNT` overhead for no gain. The fire system reads `self.active_wieldable` the same way the movement tick reads the camera. The seam: a future inventory plan replaces this field with an inventory query behind the same chokepoint, without touching the fire system.
