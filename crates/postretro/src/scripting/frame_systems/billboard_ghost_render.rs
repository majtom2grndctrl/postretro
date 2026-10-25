// Billboard ghost render collector: each sprite-bodied projectile ghost at its mapped
// position, culled at the mapped cell, with its look through the per-ghost flash clamp.
// See: context/lib/rendering_pipeline.md §7.9, §7.4, §7.8 · context/lib/entity_model.md §7d

use std::collections::HashMap;

use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_level_loader::LevelWorld;
use postretro_render_cpu::flash_clamp::GhostLookClamp;
use postretro_render_cpu::flash_limiter::LimiterFrameUniform;
use postretro_render_cpu::ghost_instance::pack_ghost_sprite_instance;
use postretro_render_cpu::mesh_pass::mesh_visible;
use postretro_visibility::VisibleCells;

use super::ghost_render::{GhostDrawKey, presented_body_look};
use super::particle_render::{ParticleRenderCollector, projectile_sprite_body};
use crate::ghost_links::GhostMember;

/// One frame's read-only inputs beside the members and their looks.
pub(crate) struct BillboardGhostFrame<'a> {
    pub world: &'a LevelWorld,
    pub visible: &'a VisibleCells,
    /// The host simulation epoch the real billboards age display copies by.
    pub presentation_tick: f64,
    /// This presented frame's limiter input, or `None` for a capture.
    pub limiter: Option<LimiterFrameUniform>,
}

/// The pose a ghost member presents at, the one its real draw uses: a
/// sprite-bodied projectile's current transform, since the billboard pass
/// draws billboards uninterpolated, and every other member's interpolated
/// transform, as the real mesh and mover draws present. Membership tests and
/// ghost draws both read it, so a billboard ghost enters, leaves and draws at
/// the mapping of where its billboard is drawn. `None` without a transform.
pub(crate) fn ghost_presented_transform(
    registry: &EntityRegistry,
    entity: EntityId,
    alpha: f32,
) -> Option<Transform> {
    // The flipbook age is unused here; any presentation tick answers whether
    // the entity is a billboard body.
    if projectile_sprite_body(registry, entity, 0.0).is_some() {
        return registry.get_component::<Transform>(entity).ok().copied();
    }
    registry.interpolated_transform(entity, alpha).ok()
}

/// The sprite a ghost last drew: a fading ghost redraws it unchanged, since its
/// projectile may already be gone.
#[derive(Debug, Clone, Copy)]
struct GhostSprite {
    /// The particle collector's resolved collection index.
    collection: usize,
    size: f32,
    rotation: f32,
    /// The body's own sprite opacity, judged with the ghost's look.
    opacity: f32,
    age: f32,
    /// Still a ghost this frame; sprites of departed ghosts are dropped.
    seen: bool,
}

/// Game-side billboard ghost collection: a sprite-bodied projectile's ghost
/// (a model-bodied one draws through the skinned ghost collector). Owns no
/// wgpu: per-collection ghost sprite bytes reach the renderer through
/// `set_billboard_ghost_draws`, batched by the collections the real billboard
/// pass registered. Unlit and always blended, whatever `render.ghosts` says.
#[derive(Debug, Default)]
pub(crate) struct BillboardGhostRenderCollector {
    /// Packed ghost sprites, indexed by the particle collector's collection.
    buffers: Vec<Vec<u8>>,
    sprites: HashMap<GhostDrawKey, GhostSprite>,
    clamp: GhostLookClamp<GhostDrawKey>,
}

impl BillboardGhostRenderCollector {
    /// Drop every cached sprite and the clamp's history: level unload. Kept
    /// byte buffers retain their capacity for the next level.
    pub(crate) fn clear(&mut self) {
        for buffer in &mut self.buffers {
            buffer.clear();
        }
        self.sprites.clear();
        self.clamp = GhostLookClamp::default();
    }

    /// Collect this frame's billboard ghosts. Collection names resolve through
    /// `particles`, the real billboard pass's registrations, so a ghost and its
    /// projectile draw from one sprite collection. No allocation once every
    /// ghost's sprite and collection buffer exist.
    pub(crate) fn collect(
        &mut self,
        registry: &EntityRegistry,
        members: &[GhostMember],
        particles: &mut ParticleRenderCollector,
        frame: &BillboardGhostFrame<'_>,
    ) {
        for buffer in &mut self.buffers {
            buffer.clear();
        }
        self.clamp.begin_frame(frame.limiter.as_ref());
        for member in members {
            let key = (member.link, member.entity);
            if !member.frozen {
                self.refresh_sprite(registry, member, particles, frame.presentation_tick);
            }
            let Some(sprite) = self.sprites.get_mut(&key) else {
                continue;
            };
            sprite.seen = true;
            let sprite = *sprite;
            // Every presented ghost passes its clamp, a sprite at opacity 0
            // included, so its history settles as any other ghost's. The
            // sprite's own opacity is judged with the link look: a fade-out
            // rides the exempt fall, a strobe is held.
            let ghost_look = presented_body_look(&mut self.clamp, member, sprite.opacity);
            let position = member.mapped.position;
            if ghost_look.opacity <= 0.0 || !mesh_visible(frame.world, frame.visible, position) {
                continue;
            }
            if self.buffers.len() <= sprite.collection {
                self.buffers.resize_with(sprite.collection + 1, Vec::new);
            }
            pack_ghost_sprite_instance(
                position,
                sprite.age,
                sprite.size,
                sprite.rotation,
                ghost_look,
                &mut self.buffers[sprite.collection],
            );
        }
        self.clamp.end_frame();
        self.sprites
            .retain(|_, sprite| std::mem::take(&mut sprite.seen));
    }

    /// Settle this frame's look clamp once the renderer knows whether the
    /// frame presented: one that presented nothing leaves the clamp as if it
    /// never ran and carries its time into the next frame.
    pub(crate) fn finish_presentation(&mut self, presented: bool) {
        self.clamp.finish_presentation(presented);
    }

    /// Each collection's packed ghost sprites this frame, by registered name.
    pub(crate) fn iter_collections<'a>(
        &'a self,
        particles: &'a ParticleRenderCollector,
    ) -> impl Iterator<Item = (&'a str, &'a [u8])> + 'a {
        self.buffers
            .iter()
            .enumerate()
            .filter(|(_, bytes)| !bytes.is_empty())
            .filter_map(|(index, bytes)| {
                Some((
                    particles.registered_collection_name(index)?,
                    bytes.as_slice(),
                ))
            })
    }

    /// Re-read a live member's billboard: its body as the real billboard pass
    /// draws it. An entity with no billboard body drops any sprite it had.
    fn refresh_sprite(
        &mut self,
        registry: &EntityRegistry,
        member: &GhostMember,
        particles: &mut ParticleRenderCollector,
        presentation_tick: f64,
    ) {
        let key = (member.link, member.entity);
        let Some((visual, age)) =
            projectile_sprite_body(registry, member.entity, presentation_tick)
        else {
            self.sprites.remove(&key);
            return;
        };
        let Some(collection) = particles.resolve_collection(&visual.collection) else {
            self.sprites.remove(&key);
            return;
        };
        let seen = self.sprites.get(&key).is_some_and(|sprite| sprite.seen);
        self.sprites.insert(
            key,
            GhostSprite {
                collection,
                size: visual.size,
                rotation: visual.rotation,
                opacity: visual.opacity,
                age,
                seen,
            },
        );
    }
}

#[cfg(test)]
#[path = "billboard_ghost_render_tests.rs"]
mod tests;
