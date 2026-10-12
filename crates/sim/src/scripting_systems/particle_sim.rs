// Frame-time presentation particle integration.
// See: context/lib/entity_model.md §8.

use glam::Vec3;
use postretro_entities::EntityRegistry;

use crate::particle_pool::ParticlePool;

/// Step newly emitted and surviving particles before this frame's collection.
/// The callback resolves gravity and push at each particle's post-step position.
pub fn tick(
    pool: &mut ParticlePool,
    registry: &EntityRegistry,
    delta: f32,
    resolve_acceleration: impl FnMut(Vec3) -> (f32, Vec3),
) {
    pool.step(registry, delta, resolve_acceleration);
}
