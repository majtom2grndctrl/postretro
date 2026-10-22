// Billboard ghost render collector: each sprite-bodied projectile ghost at its mapped
// position, culled at the mapped cell, with its look through the per-ghost flash clamp.
// See: context/lib/rendering_pipeline.md §7.9, §7.4, §7.8 · context/lib/entity_model.md §7d

use std::collections::HashMap;

use postretro_entities::EntityRegistry;
use postretro_level_loader::LevelWorld;
use postretro_render_cpu::flash_clamp::GhostLookClamp;
use postretro_render_cpu::flash_limiter::LimiterFrameUniform;
use postretro_render_cpu::ghost_instance::pack_ghost_sprite_instance;
use postretro_render_cpu::mesh_pass::mesh_visible;
use postretro_visibility::VisibleCells;

use super::ghost_render::{GhostDrawKey, presented_look};
use super::particle_render::{ParticleRenderCollector, projectile_sprite_body};
use crate::ghost_links::{GhostLook, GhostMember};

/// One frame's read-only inputs beside the members and their looks.
pub(crate) struct BillboardGhostFrame<'a> {
    pub world: &'a LevelWorld,
    pub visible: &'a VisibleCells,
    /// The host simulation epoch the real billboards age display copies by.
    pub presentation_tick: f64,
    /// This presented frame's limiter input, or `None` for a capture.
    pub limiter: Option<LimiterFrameUniform>,
}

/// The sprite a ghost last drew: a fading ghost redraws it unchanged, since its
/// projectile may already be gone.
#[derive(Debug, Clone, Copy)]
struct GhostSprite {
    /// The particle collector's resolved collection index.
    collection: usize,
    size: f32,
    rotation: f32,
    /// The body's own sprite opacity, under the ghost's presented opacity.
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
        look: impl Fn(u16) -> GhostLook,
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
            let mut ghost_look = presented_look(&mut self.clamp, member, look(member.link));
            ghost_look.opacity *= sprite.opacity;
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
