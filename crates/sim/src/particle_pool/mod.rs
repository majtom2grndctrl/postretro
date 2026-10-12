// Bounded presentation particles, immutable spawn parameters and live emitter spin.
// See: context/lib/entity_model.md §8.

use std::collections::HashMap;
use std::sync::Arc;

use glam::Vec3;
use postretro_entities::components::billboard_emitter::BillboardEmitterComponent;
use postretro_entities::{EntityId, EntityRegistry};

use crate::scripting_systems::eval_curve;

pub const MAX_LIVE_PARTICLES: usize = 65_536;
pub const IMPACT_PARTICLE_COUNT: usize = 9;
const OVERFLOW_WARNING_INTERVAL: f64 = 1.0;

/// Immutable values sampled at a successful spawn. Spin is deliberately absent:
/// it follows the live emitter, while all these values belong to the particle.
#[derive(Clone, Debug, PartialEq)]
pub struct ParticleParameters {
    pub lifetime: f32,
    pub buoyancy: f32,
    pub drag: f32,
    pub size_curve: Arc<[f32]>,
    pub opacity_curve: Arc<[f32]>,
    pub collection: usize,
    pub tint: [f32; 3],
}

/// Read-only rendering snapshot. Pool order is unspecified after expiry.
#[derive(Clone, Copy, Debug)]
pub struct ParticleView {
    pub position: Vec3,
    pub velocity: Vec3,
    pub age: f32,
    pub lifetime: f32,
    pub size: f32,
    pub opacity: f32,
    pub rotation: f32,
    pub collection: usize,
    pub tint: [f32; 3],
    pub emitter: Option<EntityId>,
}

struct ParameterBlock {
    parameters: ParticleParameters,
    live: usize,
    spin_slot: Option<usize>,
}

struct SpinSlot {
    emitter: EntityId,
    rate: f32,
    live: usize,
    current_block: Option<usize>,
    active_index: usize,
}

/// Game-layer-owned SoA storage. Reserving the bound once keeps admission and
/// swap-removal allocation-free, even at a new emitter/block high-water mark.
/// Only a newly minted block clones curve handles; particles hold compact indices.
pub struct ParticlePool {
    positions: Vec<Vec3>,
    velocities: Vec<Vec3>,
    ages: Vec<f32>,
    sizes: Vec<f32>,
    opacities: Vec<f32>,
    rotations: Vec<f32>,
    block_indices: Vec<u32>,
    blocks: Vec<Option<ParameterBlock>>,
    free_blocks: Vec<usize>,
    spins: Vec<Option<SpinSlot>>,
    free_spins: Vec<usize>,
    active_spins: Vec<usize>,
    emitter_spins: HashMap<EntityId, usize>,
    current_time: f64,
    last_overflow_warning: Option<f64>,
}

impl Default for ParticlePool {
    fn default() -> Self {
        Self::new()
    }
}

impl ParticlePool {
    pub fn new() -> Self {
        let mut blocks = Vec::with_capacity(MAX_LIVE_PARTICLES + IMPACT_PARTICLE_COUNT);
        let size_curve: Arc<[f32]> = Arc::from([0.18, 0.12, 0.0]);
        let opacity_curve: Arc<[f32]> = Arc::from([1.0, 0.7, 0.0]);
        for index in 0..IMPACT_PARTICLE_COUNT {
            blocks.push(Some(ParameterBlock {
                parameters: ParticleParameters {
                    lifetime: 0.18 * (0.8 + index as f32 * 0.025),
                    buoyancy: 0.0,
                    drag: 4.0,
                    size_curve: Arc::clone(&size_curve),
                    opacity_curve: Arc::clone(&opacity_curve),
                    collection: 0,
                    tint: [1.0, 0.88, 0.45],
                },
                live: 0,
                spin_slot: None,
            }));
        }
        Self {
            positions: Vec::with_capacity(MAX_LIVE_PARTICLES),
            velocities: Vec::with_capacity(MAX_LIVE_PARTICLES),
            ages: Vec::with_capacity(MAX_LIVE_PARTICLES),
            sizes: Vec::with_capacity(MAX_LIVE_PARTICLES),
            opacities: Vec::with_capacity(MAX_LIVE_PARTICLES),
            rotations: Vec::with_capacity(MAX_LIVE_PARTICLES),
            block_indices: Vec::with_capacity(MAX_LIVE_PARTICLES),
            blocks,
            free_blocks: Vec::with_capacity(MAX_LIVE_PARTICLES),
            spins: Vec::with_capacity(MAX_LIVE_PARTICLES),
            free_spins: Vec::with_capacity(MAX_LIVE_PARTICLES),
            active_spins: Vec::with_capacity(MAX_LIVE_PARTICLES),
            emitter_spins: HashMap::with_capacity(MAX_LIVE_PARTICLES),
            current_time: 0.0,
            last_overflow_warning: None,
        }
    }

    /// Level setup supplies the resolved collection before any particle exists.
    pub fn configure_impact_collection(&mut self, collection: usize) {
        if self.blocks[0]
            .as_ref()
            .expect("static impact block")
            .parameters
            .collection
            == collection
        {
            return;
        }
        assert!(
            self.is_empty(),
            "impact collection changes require an empty pool"
        );
        for block in &mut self.blocks[..IMPACT_PARTICLE_COUNT] {
            block
                .as_mut()
                .expect("static impact block")
                .parameters
                .collection = collection;
        }
    }

    /// Clock injection keeps overflow throttling deterministic and off wall time.
    pub fn set_current_time(&mut self, current_time: f64) {
        self.current_time = current_time;
    }

    pub fn len(&self) -> usize {
        self.positions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    pub fn live_count(&self, emitter: EntityId) -> usize {
        self.emitter_spins.get(&emitter).map_or(0, |&slot| {
            self.spins[slot].as_ref().expect("live spin slot").live
        })
    }

    /// Includes the permanent impact blocks, even when no impacts are live.
    pub fn parameter_block_count(&self) -> usize {
        self.blocks.len() - self.free_blocks.len()
    }

    pub fn spin_slot_count(&self) -> usize {
        self.active_spins.len()
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = ParticleView> + '_ {
        (0..self.len()).map(|index| {
            let block = self.blocks[self.block_indices[index] as usize]
                .as_ref()
                .expect("live block");
            let parameters = &block.parameters;
            ParticleView {
                position: self.positions[index],
                velocity: self.velocities[index],
                age: self.ages[index],
                lifetime: parameters.lifetime,
                size: self.sizes[index],
                opacity: self.opacities[index],
                rotation: self.rotations[index],
                collection: parameters.collection,
                tint: parameters.tint,
                emitter: block
                    .spin_slot
                    .map(|slot| self.spins[slot].as_ref().expect("live spin slot").emitter),
            }
        })
    }

    /// Drops the newest spawn at the global budget; existing particles survive.
    /// Producers enforce their per-emitter headroom using `live_count` first.
    pub fn spawn_emitter(
        &mut self,
        emitter: EntityId,
        parameters: &ParticleParameters,
        position: Vec3,
        velocity: Vec3,
        spin: f32,
    ) -> bool {
        if !self.admit() {
            return false;
        }
        let slot = self.emitter_slot(emitter, spin);
        let current = self.spins[slot]
            .as_ref()
            .expect("emitter slot")
            .current_block;
        let block = match current {
            Some(index)
                if self.blocks[index]
                    .as_ref()
                    .expect("current block")
                    .parameters
                    == *parameters =>
            {
                index
            }
            _ => self.mint_block(slot, parameters),
        };
        self.push(block, position, velocity, 0.0);
        true
    }

    /// Impacts use permanent blocks and therefore never mint or clone curves.
    pub fn spawn_impact(&mut self, position: Vec3, velocity: Vec3, index: usize) -> bool {
        assert!(
            index < IMPACT_PARTICLE_COUNT,
            "impact particle index out of range"
        );
        if !self.admit() {
            return false;
        }
        self.push(index, position, velocity, index as f32 * 0.73);
        true
    }

    fn admit(&mut self) -> bool {
        if self.len() < MAX_LIVE_PARTICLES {
            return true;
        }
        if self
            .last_overflow_warning
            .is_none_or(|last| self.current_time - last >= OVERFLOW_WARNING_INTERVAL)
        {
            log::warn!(
                "[Particles] live budget {MAX_LIVE_PARTICLES} reached; dropping newest particles"
            );
            self.last_overflow_warning = Some(self.current_time);
        }
        false
    }

    fn emitter_slot(&mut self, emitter: EntityId, rate: f32) -> usize {
        if let Some(&index) = self.emitter_spins.get(&emitter) {
            self.spins[index].as_mut().expect("live spin slot").rate = rate;
            return index;
        }
        let slot = SpinSlot {
            emitter,
            rate,
            live: 0,
            current_block: None,
            active_index: self.active_spins.len(),
        };
        let index = if let Some(index) = self.free_spins.pop() {
            self.spins[index] = Some(slot);
            index
        } else {
            self.spins.push(Some(slot));
            self.spins.len() - 1
        };
        self.active_spins.push(index);
        self.emitter_spins.insert(emitter, index);
        index
    }

    fn mint_block(&mut self, slot: usize, parameters: &ParticleParameters) -> usize {
        let block = ParameterBlock {
            parameters: parameters.clone(),
            live: 0,
            spin_slot: Some(slot),
        };
        let index = if let Some(index) = self.free_blocks.pop() {
            self.blocks[index] = Some(block);
            index
        } else {
            self.blocks.push(Some(block));
            self.blocks.len() - 1
        };
        self.spins[slot]
            .as_mut()
            .expect("live spin slot")
            .current_block = Some(index);
        index
    }

    fn push(&mut self, block: usize, position: Vec3, velocity: Vec3, rotation: f32) {
        let block_data = self.blocks[block].as_mut().expect("spawn block");
        block_data.live += 1;
        if let Some(slot) = block_data.spin_slot {
            self.spins[slot].as_mut().expect("live spin slot").live += 1;
        }
        self.positions.push(position);
        self.velocities.push(velocity);
        self.ages.push(0.0);
        self.sizes
            .push(eval_curve(&block_data.parameters.size_curve, 0.0));
        self.opacities
            .push(eval_curve(&block_data.parameters.opacity_curve, 0.0));
        self.rotations.push(rotation);
        self.block_indices.push(block as u32);
    }

    /// Integrate once on frame time. Environment resolution samples the advanced
    /// position, gravity is buoyancy-scaled, and push is independent of buoyancy.
    pub fn step(
        &mut self,
        registry: &EntityRegistry,
        delta: f32,
        mut resolve_acceleration: impl FnMut(Vec3) -> (f32, Vec3),
    ) {
        // One generational lookup per emitter, never per particle. Missing
        // emitters leave the sampled rate intact until their last orphan dies.
        for &index in &self.active_spins {
            let slot = self.spins[index].as_mut().expect("active spin slot");
            if let Ok(emitter) = registry.get_component::<BillboardEmitterComponent>(slot.emitter) {
                slot.rate = emitter.spin_rate;
            }
        }
        let mut index = 0;
        while index < self.len() {
            self.ages[index] += delta;
            self.positions[index] += self.velocities[index] * delta;
            let (gravity, push) = resolve_acceleration(self.positions[index]);
            let block_index = self.block_indices[index] as usize;
            let block = self.blocks[block_index].as_ref().expect("live block");
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
            if self.ages[index] >= parameters.lifetime {
                self.remove(index);
            } else {
                index += 1;
            }
        }
    }

    fn remove(&mut self, index: usize) {
        let block_index = self.block_indices.swap_remove(index) as usize;
        self.positions.swap_remove(index);
        self.velocities.swap_remove(index);
        self.ages.swap_remove(index);
        self.sizes.swap_remove(index);
        self.opacities.swap_remove(index);
        self.rotations.swap_remove(index);
        let block = self.blocks[block_index]
            .as_mut()
            .expect("removed particle block");
        block.live -= 1;
        let slot_index = block.spin_slot;
        let free_block = block.live == 0 && block_index >= IMPACT_PARTICLE_COUNT;
        if let Some(slot_index) = slot_index {
            let slot = self.spins[slot_index]
                .as_mut()
                .expect("removed particle spin slot");
            slot.live -= 1;
            // Invalidate before recycling: the previous emitter must never
            // treat a subsequent tenant of this block as its own parameters.
            if free_block && slot.current_block == Some(block_index) {
                slot.current_block = None;
            }
            if slot.live == 0 {
                let slot = self.spins[slot_index].take().expect("empty spin slot");
                self.emitter_spins.remove(&slot.emitter);
                self.active_spins.swap_remove(slot.active_index);
                if let Some(&moved) = self.active_spins.get(slot.active_index) {
                    self.spins[moved]
                        .as_mut()
                        .expect("moved spin slot")
                        .active_index = slot.active_index;
                }
                self.free_spins.push(slot_index);
            }
        }
        if free_block {
            self.blocks[block_index] = None;
            self.free_blocks.push(block_index);
        }
    }

    /// Retains all reserved storage and permanent impact blocks across levels.
    pub fn clear(&mut self) {
        self.positions.clear();
        self.velocities.clear();
        self.ages.clear();
        self.sizes.clear();
        self.opacities.clear();
        self.rotations.clear();
        self.block_indices.clear();
        self.blocks.truncate(IMPACT_PARTICLE_COUNT);
        for block in &mut self.blocks {
            block.as_mut().expect("static impact block").live = 0;
        }
        self.free_blocks.clear();
        self.spins.clear();
        self.free_spins.clear();
        self.active_spins.clear();
        self.emitter_spins.clear();
        self.current_time = 0.0;
        self.last_overflow_warning = None;
    }
}

#[cfg(test)]
mod environment_tests;
#[cfg(test)]
mod tests;
