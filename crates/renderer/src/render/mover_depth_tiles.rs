// Mover depth live tiles, per frame: bring every live tile a moved mover
// covers to this frame's t′ and point its probe's reference at it, on the
// upload batch that precedes the frame's draws. Runs once per frame at the
// SH drain, the renderer's one pre-compose boundary, so direct and bounce
// compose and the live tiles all read the same t′ (P25).
// See: context/lib/rendering_pipeline.md §4 (Direction-aware probe
// visibility, Mover state lighting)

use super::Renderer;
use super::level_resource_set::LevelResourceSet;
use super::mover_light_state::MoverLightState;
use super::sh_depth_tile_rewrite::DepthTexels;
use super::sh_streaming::{ShResidencyDrainError, ShResidencyState};
use super::sh_volume::ShVolumeResources;
use super::uploads::UploadQueue;

impl Renderer {
    /// Write this frame's live tiles and reference moves on the published
    /// level. Nothing without a level: a hand-over before install is dropped
    /// by the state table, and install resets it (P27, P30).
    pub(super) fn write_mover_depth_tiles(&mut self) -> Result<(), ShResidencyDrainError> {
        let Self {
            device,
            queue,
            full,
            ..
        } = self;
        let Some(full) = full.as_mut() else {
            return Ok(());
        };
        let Some(level) = full.level.as_mut() else {
            return Ok(());
        };
        level.write_mover_depth_tiles(device, queue, full.mover_light_states.states())
    }
}

impl LevelResourceSet {
    /// [`Renderer::write_mover_depth_tiles`] on this set.
    pub(in crate::render) fn write_mover_depth_tiles(
        &mut self,
        device: &wgpu::Device,
        queue: &UploadQueue,
        states: &[MoverLightState],
    ) -> Result<(), ShResidencyDrainError> {
        write_mover_depth_tiles(
            &mut self.sh_volume_resources,
            self.sh_streaming.as_mut(),
            device,
            queue,
            states,
        )
    }
}

/// Plan one frame's live tiles from `states` and stage them, then move the
/// probe references the plan names. A whole-loaded level stages its
/// reference texels on the frame batch with the tiles; a streamed one
/// (`streaming`) flushes them now, after the drain that already ran.
pub(in crate::render) fn write_mover_depth_tiles(
    sh: &mut ShVolumeResources,
    mut streaming: Option<&mut ShResidencyState>,
    device: &wgpu::Device,
    queue: &UploadQueue,
    states: &[MoverLightState],
) -> Result<(), ShResidencyDrainError> {
    let ShVolumeResources {
        mover_channel,
        depth_tiles,
        probe_indirection_words,
        grid_dimensions,
        depth_moment_texture,
        ..
    } = sh;
    mover_channel.set_states(states);
    let changes = mover_channel.plan_depth(|first, count, bytes| {
        depth_tiles.write_live_tiles(queue, first, count, bytes);
    });
    if changes.is_empty() {
        return Ok(());
    }
    let mut texels = DepthTexels {
        depth_tiles,
        probe_indirection_words,
        grid_dimensions: *grid_dimensions,
        depth_moment_texture,
    };
    for &(probe, tile) in changes {
        // Depth records name valid probes of this lattice and live tiles
        // inside this texture (both checked at load), so a refusal here is a
        // broken install, not data.
        texels
            .set_reference(streaming.as_deref_mut(), queue, probe, tile)
            .map_err(|error| {
                ShResidencyDrainError::InvalidBatch(format!(
                    "mover depth live tile reference for probe {probe}: {error}"
                ))
            })?;
    }
    if let Some(streaming) = streaming {
        streaming.flush_depth_tile_texels_now(device, queue, texels.depth_tiles.references())?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "mover_depth_tiles_tests.rs"]
mod tests;
