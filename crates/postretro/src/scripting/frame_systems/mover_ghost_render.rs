// Mover ghost render collector: each mover ghost at its link's mapped model, culled at
// the mapped bounds, with its look through the per-ghost flash clamp.
// See: context/lib/rendering_pipeline.md §7.9, §7.8 · context/lib/entity_model.md §7d

use postretro_entities::{EntityRegistry, KinematicMoverComponent};
use postretro_level_loader::LevelWorld;
use postretro_render_cpu::flash_clamp::GhostLookClamp;
use postretro_render_cpu::flash_limiter::LimiterFrameUniform;
use postretro_render_cpu::ghost_instance::KinematicGhostInstance;
use postretro_visibility::VisibleCells;

use super::ghost_render::{GhostDrawKey, presented_look};
use crate::ghost_links::{GhostCategory, GhostMember};
use crate::runtime_movers::{KinematicMoverRenderCollector, transform_matrix};

/// One frame's read-only inputs beside the members and their looks.
pub(crate) struct MoverGhostFrame<'a> {
    pub world: &'a LevelWorld,
    pub visible: &'a VisibleCells,
    /// This frame's mover collector, already collected: its mover bounds and
    /// visible-cell bounds cull each ghost as the real mover culls.
    pub movers: &'a KinematicMoverRenderCollector,
    /// This presented frame's limiter input, or `None` for a capture.
    pub limiter: Option<LimiterFrameUniform>,
}

/// Game-side mover ghost collection. Owns no wgpu: it hands the renderer
/// [`KinematicGhostInstance`]s through `set_kinematic_ghost_draws`.
///
/// Per mover ghost: the model is the link's mapping applied to the presented
/// pose membership holds (live, or the last in-volume pose while the ghost
/// fades after a disarm or a category change), so a ghost never draws at the
/// mapping of a pose outside the volume. Movers never despawn mid-level, so
/// the pose alone carries a frozen ghost; nothing is cached. One instance per
/// ghost, never deduplicated by mover id: a mover in two links draws twice.
/// Mover ghosts never reach shadow casters, occluder bounds or SH sample
/// regions; those stay the real mover's.
#[derive(Debug, Default)]
pub(crate) struct MoverGhostRenderCollector {
    instances: Vec<KinematicGhostInstance>,
    clamp: GhostLookClamp<GhostDrawKey>,
}

impl MoverGhostRenderCollector {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Drop the clamp's history and this frame's instances: level unload.
    pub(crate) fn clear(&mut self) {
        *self = Self::new();
    }

    /// Collect this frame's mover ghosts. No allocation once the instance list
    /// has grown to the frame's ghost count.
    pub(crate) fn collect(
        &mut self,
        registry: &EntityRegistry,
        members: &[GhostMember],
        frame: &MoverGhostFrame<'_>,
    ) {
        self.instances.clear();
        self.clamp.begin_frame(frame.limiter.as_ref());
        for member in members {
            if member.category != GhostCategory::Movers {
                continue;
            }
            let Ok(mover) = registry.get_component::<KinematicMoverComponent>(member.entity) else {
                continue;
            };
            let ghost_look = presented_look(&mut self.clamp, member);
            if ghost_look.opacity <= 0.0 {
                continue;
            }
            let model = member.mapping.matrix() * transform_matrix(member.source);
            if !frame
                .movers
                .visible_at(frame.world, frame.visible, mover.mover_id, model)
            {
                continue;
            }
            self.instances.push(KinematicGhostInstance {
                mover_id: mover.mover_id,
                transform: model,
                look: ghost_look,
            });
        }
        self.clamp.end_frame();
    }

    /// The mover ghost instances to hand the renderer this frame.
    pub(crate) fn instances(&self) -> &[KinematicGhostInstance] {
        &self.instances
    }
}

#[cfg(test)]
#[path = "mover_ghost_render_tests.rs"]
mod tests;
