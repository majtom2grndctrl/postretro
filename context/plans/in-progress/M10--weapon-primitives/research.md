# Weapon Primitives — Research Notes

Ground-truth code inventory gathered during drafting. Not part of the spec; kept for implementer orientation. Confirm against source before relying on any signature — these were read at draft time.

## Execution model (the anchor)

"Authored in the SDK as a reference behavior" means **script declares parameters, Rust runs the logic** — confirmed against M7 movement:

- `content/dev/scripts/player.ts` only calls `defineEntity({...})` with a `movement` block of tuning values. Zero algorithm logic.
- `crates/postretro/src/movement/mod.rs` `tick()` runs the full per-tick simulation in Rust; called from `main.rs` `run_movement_tick()`. No VM involvement during gameplay.
- `PlayerMovementDescriptor` (script wire shape) → `PlayerMovementComponent::from_descriptor()` materializes params at spawn.

Weapons mirror this: weapon descriptor (params) → weapon component → Rust fire tick.

## What exists (reuse)

- **Input.** `Action::Shoot`, `AltFire`, `Reload` already defined in `crates/postretro/src/input/types.rs` — **no handlers read them yet.** `ButtonState { Pressed, Held, Released, Inactive }`, `is_active()` (Pressed|Held). Per-frame `ActionSnapshot`.
- **Camera / aim.** `crates/postretro/src/camera.rs` `Camera { position, yaw, pitch }`, `forward()`/`right()` (yaw-only). Pitch-inclusive direction math exists test-only (~camera.rs:85) — needs promotion to a real view-ray method.
- **Collision.** `crates/postretro/src/collision/mod.rs` `CollisionWorld::cast_ray(world, origin, dir, max_toi) -> Option<RayIntersection>` (pub(crate)). `RayIntersection` carries `time_of_impact` + `normal`. Hit point = `origin + dir * toi`.
- **Entity registry.** `crates/postretro/src/scripting/registry.rs` `EntityRegistry::{spawn, try_spawn, set_component, get_component, despawn, query_by_component_and_tag}`. `ComponentKind` enum: Transform, Light, BillboardEmitter, ParticleState, SpriteVisual, FogVolume, PlayerMovement. `Component` trait with `const KIND`.
- **Entity descriptors / FGD.** `EntityTypeDescriptor { canonical_name, light, emitter, movement }` (`data_descriptors.rs`). SDK `defineEntity()` (`sdk/lib/data_script.ts`). FGD at `sdk/TrenchBroom/postretro.fgd`. Built-in classname dispatch in `scripting/builtins/mod.rs` (only `billboard_emitter` registered today); script archetypes via data-archetype fallback. Typedefs via `cargo run -p postretro --bin gen-script-types`; drift test in `cargo test`.
- **Particles / billboards.** Continuous emitters: `BillboardEmitterComponent`, particle sim in Rust, `setEmitterRate` reaction (`rate=0` dormant). Per-emitter cap `MAX_SPRITES = 512`. Particles carry lifetime.

## Gaps (must build)

- **No weapon state.** No weapon component, no descriptor fields. `ComponentKind` has no `Weapon`.
- **No damage/health.** No health component, no damage event type, no kill path. `DamageSource` reference script is specced but not in repo; its `"damage"` event has no engine consumer. (Health + kill belong to the enemy-entity plan, not here.)
- **No hit→material lookup.** `cast_ray` returns no triangle index, so `world.face_meta[i].material` is unreachable from a hit. `Material` enum + `FaceMeta.material` exist (baked). Material-aware impacts need a collision-API extension — deferred.
- **No one-shot effect spawn.** Emitters are persistent + map-placed or level-load-spawned and activated via `setEmitterRate`. No transient "spawn a burst at a world point then clean up" mechanism. The impact effect needs one.
