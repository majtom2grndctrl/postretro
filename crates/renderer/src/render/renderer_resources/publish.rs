// Publishing and releasing level sets: publish makes a built set the one
// frames read and runs every level-boundary reset there (never at build);
// release drops exactly one set and creates nothing; both share the
// extent-bound group build and rebuild.
// See: context/plans/in-progress/level-load-pipeline/index.md (Per-load renderer resources, Settings and scene extent)

use super::super::level_resource_set::{
    LevelExtentGroups, LevelSetId, LevelShadowTier, PreviousLevelSet,
};
use super::super::renderer_types::FullRenderer;
use super::*;
use postretro_render_cpu::render_extent::Extent;

/// Build one set's extent-bound groups at `extent`: the spot pool's group 5
/// over its tier's textures, the SDF shadow factor target and scene depth,
/// and the fog scatter target at `fog_pixel_scale` with its groups. Reads the
/// renderer's scene targets; writes nothing renderer-wide.
pub(super) fn build_level_extent_groups(
    device: &wgpu::Device,
    full: &FullRenderer,
    shadow: &LevelShadowTier,
    extent: Extent,
    fog_pixel_scale: u32,
) -> LevelExtentGroups {
    let cube_sampling_view = full.cube_shadow_pool.as_ref().map(|p| &p.sampling_view);
    LevelExtentGroups {
        extent,
        spot_shadow: shadow.tier.spot.bind_group(
            device,
            &full.spot_shadow_bgl,
            &full.sdf_shadow_pass.shadow_view,
            &full.depth_view,
            cube_sampling_view,
        ),
        fog: full.fog.build_target(
            device,
            extent.width,
            extent.height,
            fog_pixel_scale,
            &full.depth_view,
        ),
    }
}

/// Rebuild every extent-bound group `level` holds over the renderer's
/// current scene targets at `extent`: the spot pool's group 5, the fog
/// scatter target and its groups, the SDF shadow group 1 and the fluid
/// reflection targets with their groups 5. Call after the renderer's scene
/// targets exist at `extent`.
pub(in crate::render) fn rebuild_level_extent_groups(
    device: &wgpu::Device,
    full: &FullRenderer,
    level: &mut LevelResourceSet,
    extent: Extent,
) {
    let cube_sampling_view = full.cube_shadow_pool.as_ref().map(|p| &p.sampling_view);
    level.extent.spot_shadow = level.shadow.tier.spot.bind_group(
        device,
        &full.spot_shadow_bgl,
        &full.sdf_shadow_pass.shadow_view,
        &full.depth_view,
        cube_sampling_view,
    );
    full.fog.resize_target(
        device,
        &mut level.extent.fog,
        extent.width,
        extent.height,
        &full.depth_view,
    );
    full.sdf_shadow_pass
        .rebind_level_extent(device, &mut level.sdf_shadow, &full.depth_view);
    level.extent.extent = extent;
}

impl Renderer {
    /// Make the built set `id` the active set, the only one frames read, and
    /// run the level-boundary work there: when the set was built at another
    /// scene extent, its extent-bound groups and fog target rebuild once
    /// (counted by [`Renderer::extent_rebuilds`]); its sampler map merges
    /// into the renderer's add-only maps; the active spot pool and depth
    /// caches point at its shadow tier, every slot unoccupied and every
    /// cache layer cold; and per-frame draw inputs the previous level's ids
    /// index clear. `previous` says what becomes of the set it replaces:
    /// retired, or kept resident (its key returned). Publishing the active
    /// set, or an unknown key, changes nothing and returns `None`.
    pub fn publish_level_set(
        &mut self,
        id: LevelSetId,
        previous: PreviousLevelSet,
    ) -> Option<LevelSetId> {
        let scene = self.scene_extent();
        let Self { device, full, .. } = self;
        let full = full
            .as_mut()
            .expect("renderer full-init must complete before full-ready paths run");
        let Some(index) = full.resident_levels.iter().position(|level| level.id == id) else {
            if full.level.as_ref().is_none_or(|level| level.id != id) {
                log::warn!("[Renderer] publish of unknown level set {id:?} ignored");
            }
            return None;
        };
        let mut level = full.resident_levels.remove(index);
        // The replaced set's reflection targets go back to it, building nothing.
        let replaced_fluid = full.fluid_surface.take_level_targets();
        let replaced_mirror = full.planar_reflection.take_level_targets();
        let extent_changed = level.extent.extent != scene;
        if extent_changed {
            rebuild_level_extent_groups(device, full, &mut level, scene);
            full.extent_rebuilds += 1;
        }
        full.samplers.absorb(&mut level.samplers);
        shadow_pools::publish_shadow_tier(full, &level.shadow);
        reset_level_frame_state(full);
        full.fluid_surface
            .lend_level_targets(std::mem::take(&mut level.fluid_targets));
        full.planar_reflection
            .lend_level_targets(std::mem::take(&mut level.mirror_targets));
        let kept = match (full.level.replace(level), previous) {
            (Some(mut replaced), PreviousLevelSet::KeepResident) => {
                replaced.fluid_targets = replaced_fluid;
                replaced.mirror_targets = replaced_mirror;
                let kept = replaced.id;
                full.resident_levels.push(replaced);
                Some(kept)
            }
            (Some(replaced), PreviousLevelSet::Retire) => {
                drop((replaced_fluid, replaced_mirror));
                full.retire_level_set(replaced);
                None
            }
            (None, _) => None,
        };
        self.reconcile_published_fluid_targets(extent_changed);
        kept
    }

    /// Drop exactly the set `id`, active or resident, and create nothing.
    /// Releasing the active set leaves no active set (frames record the
    /// empty scene) and clears the per-frame inputs that indexed it; every
    /// other set stays bound and drawable. Returns whether a set was dropped.
    pub fn release_level_set(&mut self, id: LevelSetId) -> bool {
        self.queue.assert_empty("level unload");
        let full = self.full_mut();
        if full.level.as_ref().is_some_and(|level| level.id == id) {
            let level = full.level.take().expect("checked above");
            full.retire_level_set(level);
            reset_level_frame_state(full);
            full.mesh_pass.release_level_resources();
            full.fluid_surface.release_level_targets();
            return true;
        }
        let Some(index) = full.resident_levels.iter().position(|level| level.id == id) else {
            return false;
        };
        let level = full.resident_levels.remove(index);
        full.retire_level_set(level);
        true
    }
}
