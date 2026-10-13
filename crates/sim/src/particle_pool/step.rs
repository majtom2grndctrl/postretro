// Chunked frame-time particle integration and expiry.
// See: context/lib/entity_model.md §8.

use super::*;

const ACCELERATION_BATCH_SIZE: usize = 64;

impl ParticlePool {
    /// Integrate once on frame time with a point resolver.
    pub fn step(
        &mut self,
        registry: &EntityRegistry,
        delta: f32,
        mut resolve_acceleration: impl FnMut(Vec3) -> (f32, Vec3),
    ) {
        self.step_batched(registry, delta, |positions, accelerations| {
            for (&position, acceleration) in positions.iter().zip(accelerations) {
                *acceleration = resolve_acceleration(position);
            }
        });
    }

    /// Resolve each chunk's advanced positions into the equally sized output.
    /// The resolver must fill every output; chunks imply no spatial coherence.
    /// Gravity is buoyancy-scaled, while push is independent of buoyancy.
    pub fn step_batched(
        &mut self,
        registry: &EntityRegistry,
        delta: f32,
        mut resolve_accelerations: impl FnMut(&[Vec3], &mut [(f32, Vec3)]),
    ) {
        // One generational lookup per emitter. Orphans retain the last sample.
        for &index in &self.active_spins {
            let slot = self.spins[index].as_mut().expect("active spin slot");
            if let Ok(emitter) = registry.get_component::<BillboardEmitterComponent>(slot.emitter) {
                slot.rate = emitter.spin_rate;
            }
        }
        let mut accelerations = [(0.0, Vec3::ZERO); ACCELERATION_BATCH_SIZE];
        for start in (0..self.len()).step_by(ACCELERATION_BATCH_SIZE) {
            let end = (start + ACCELERATION_BATCH_SIZE).min(self.len());
            for index in start..end {
                self.ages[index] += delta;
                self.positions[index] += self.velocities[index] * delta;
            }
            resolve_accelerations(
                &self.positions[start..end],
                &mut accelerations[..end - start],
            );
            for (offset, &(gravity, push)) in accelerations[..end - start].iter().enumerate() {
                let index = start + offset;
                let block = self.blocks[self.block_indices[index] as usize]
                    .as_ref()
                    .expect("live block");
                let parameters = &block.parameters;
                let velocity = &mut self.velocities[index];
                velocity.y += gravity * -parameters.buoyancy * delta;
                *velocity += push * delta;
                *velocity *= (1.0 - parameters.drag * delta).max(0.0);
                let normalized_age =
                    (self.ages[index] / parameters.lifetime.max(f32::MIN_POSITIVE)).clamp(0.0, 1.0);
                self.sizes[index] = eval_curve(&parameters.size_curve, normalized_age);
                self.opacities[index] = eval_curve(&parameters.opacity_curve, normalized_age);
                if let Some(slot) = block.spin_slot {
                    self.rotations[index] +=
                        self.spins[slot].as_ref().expect("live spin slot").rate * delta;
                }
            }
        }
        // Defer compaction until every chunk has stepped. Removing backwards
        // only moves an already-stepped survivor, never an unqueried particle.
        for index in (0..self.len()).rev() {
            let block = self.blocks[self.block_indices[index] as usize]
                .as_ref()
                .expect("live block");
            if self.ages[index] >= block.parameters.lifetime {
                self.remove(index);
            }
        }
    }
}
