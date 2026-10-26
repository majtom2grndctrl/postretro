// Planned mesh frame: instance placements, per-model draw groups, budget plans and cap occupancy.
// See: context/lib/rendering_pipeline.md §9, §7.3a

use glam::{Mat4, Vec4};

use postretro_entities::PoseInputs;
use postretro_model::ModelHandle;
use postretro_model::sample_params::{CaptureInstruction, MeshSampleParams};
use postretro_render_data::cone_frustum::{Aabb, aabb_intersects_frustum};

use super::input::{MeshPaletteCacheKey, MirrorReach};
use crate::ghost_instance::GhostInstanceLook;
// Referenced only by intra-doc links below.
#[cfg(doc)]
use super::input::MeshInstanceInput;

/// One instance's resolved placement in the frame plan: its world transform, the
/// base index of its contiguous palette run in the shared buffer, its phase seed
/// (carried through so the GPU layer can sample its clip into the run at a
/// per-instance phase), and its model's conservative LOCAL-space pose envelope.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedInstance {
    pub transform: Mat4,
    /// Carried unchanged into the renderer's 80-byte skinned instance record.
    pub shadow_bias_scale: f32,
    pub palette_base: u32,
    pub phase_seed: u32,
    pub palette_cache_key: MeshPaletteCacheKey,
    /// The instance model's conservative LOCAL-space pose envelope, stamped from
    /// the renderer's model cache at plan time. It encloses clip samples,
    /// crossfades, snapshots, and pose modifiers. Shadow and sampled-SH planning
    /// transform it by `transform`; the GPU draw never reads it.
    pub bounds: Aabb,
    /// Resolved per-frame sample parameters carried verbatim from the collector
    /// — the GPU layer feeds these to the pose sampler (single / blended /
    /// snapshot-blended), replacing the hardcoded first-clip-at-render-clock path.
    pub sample: MeshSampleParams,
    /// Carried verbatim from [`MeshInstanceInput::pose_inputs`] for palette sampling.
    pub pose_inputs: Option<PoseInputs>,
    /// One-time `"smooth"`-interrupt capture instruction for this frame, if any.
    /// The GPU layer evaluates it into the snapshot store (idempotent by tag)
    /// before sampling this frame's pose.
    pub capture: Option<CaptureInstruction>,
    /// Animation time-slicing decision, carried verbatim from the
    /// instance input. `true` → the pass samples this instance's pose AND
    /// refreshes its palette cache; `false` → the pass re-uploads the cached
    /// palette run with no sampling. A renderer-side cache MISS upgrades a
    /// `false` to a resample regardless (the collector cannot see cache state),
    /// so a culled instance re-entering view never shows a stale pose. `Copy`
    /// bool — no per-instance heap.
    pub resample: bool,
    /// Carried verbatim from [`MeshInstanceInput::forward_visible`]. `record_draws`
    /// filters this flag so shadow-only and retained off-PVS casters do not draw
    /// in the color pass.
    pub forward_visible: bool,
    /// Carried verbatim from [`MeshInstanceInput::dynamic_shadow_visible`].
    /// Dynamic shadow passes filter on this while promoted-static passes may use
    /// every retained planned instance.
    pub dynamic_shadow_visible: bool,
    /// Carried from [`MeshInstanceInput::mirror`], normalized over the holder
    /// group like the flags above. The mirror pass draws [`MirrorReach::drawn`]
    /// instances; promoted-static depth skips [`MirrorReach::Only`] ones.
    pub mirror: MirrorReach,
    /// A ghost instance's presented look, packed into the instance record's
    /// free lanes; `None` for world and viewmodel instances.
    pub ghost_look: Option<GhostInstanceLook>,
}

/// All instances of one model, batched for a single instanced `draw_indexed` per
/// submesh. The instances are contiguous in the per-frame instance SSBO, so the
/// draw uses `instance_offset..instance_offset + instances.len()` and the shader
/// reads each instance via `@builtin(instance_index)`.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelDrawGroup {
    pub model: ModelHandle,
    /// Offset of this group's first instance in the flat instance SSBO.
    pub instance_offset: u32,
    pub instances: Vec<PlannedInstance>,
}

/// The per-frame skinned-mesh draw plan: one group per distinct model (in
/// first-seen order), the flat instance count, and how many instances were
/// dropped because either budget was exhausted (palette slots or the per-frame
/// instance cap).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshFramePlan {
    pub groups: Vec<ModelDrawGroup>,
    /// Total planned instances across all groups (== sum of group lengths). The
    /// instance SSBO is filled densely in group order, so a group's instances
    /// occupy `instance_offset..instance_offset + len`.
    pub instance_count: u32,
    /// Instances dropped because EITHER their palette run would exceed
    /// `MAX_PALETTE_ENTRIES` OR the instance count would reach `MAX_INSTANCES`
    /// (the per-frame instance SSBO size). The caller rate-limits a warning when
    /// this is non-zero.
    pub dropped: u32,
}

/// The plans sharing this frame's palette and instance SSBOs.
///
/// Budget admission is global across the plans, in this order:
/// 1. For each of three priorities (forward-visible, dynamic-shadow-only,
///    promoted-static-only), the world groups, then the viewmodel groups.
/// 2. Ghosts, after every real main-view group.
/// 3. World instances only the planar mirror reaches.
///
/// An overflow drops in the reverse order: mirror-only world first, then
/// ghosts, before any real mesh the main view reaches. Dense instance offsets
/// place the viewmodel after the world plan and ghosts last; no plan owns a
/// separate GPU buffer. Only `world` reaches shadow depth and SH sampling.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshFramePlans {
    pub world: MeshFramePlan,
    pub viewmodel: MeshFramePlan,
    pub ghost: MeshFramePlan,
    /// The part of `world.dropped` that only the planar mirror reached.
    pub dropped_mirror_only: u32,
    /// The part of `world.instance_count` that only the planar mirror reaches:
    /// exactly the instances admitted at the planner's last priority.
    pub planned_mirror_only: u32,
}

/// The frame's mesh-instance cap occupancy split by admission rank
/// (`rendering_pipeline.md` §7.3a, Pool rank): instances admitted for the
/// main view (forward draws, shadow casters, viewmodels and ghosts), those
/// only the planar mirror reaches, and what the cap dropped of each.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MeshCapOccupancy {
    pub main: u32,
    pub mirror_only: u32,
    pub dropped_main: u32,
    pub dropped_mirror_only: u32,
}

impl MeshFramePlans {
    /// This frame's cap occupancy, from the plans alone.
    pub fn occupancy(&self) -> MeshCapOccupancy {
        let planned =
            self.world.instance_count + self.viewmodel.instance_count + self.ghost.instance_count;
        let mirror_only = self.planned_mirror_only;
        let dropped = self
            .world
            .dropped
            .saturating_add(self.viewmodel.dropped)
            .saturating_add(self.ghost.dropped);
        MeshCapOccupancy {
            main: planned - mirror_only,
            mirror_only,
            dropped_main: dropped - self.dropped_mirror_only,
            dropped_mirror_only: self.dropped_mirror_only,
        }
    }
}

/// World-space bounds for receivers admitted by the shared mesh-frame budget:
/// forward draws and the planar mirror's draws, which sample the same SH.
/// Shadow-only instances and inputs rejected for cache absence or overflow
/// never enter this iterator.
pub fn planned_forward_sample_bounds(
    plans: &MeshFramePlans,
    include_viewmodels: bool,
) -> impl Iterator<Item = Aabb> + '_ {
    plans
        .world
        .groups
        .iter()
        .chain(if include_viewmodels {
            plans.viewmodel.groups.as_slice()
        } else {
            &[]
        })
        .flat_map(|group| group.instances.iter())
        .filter(|instance| instance.forward_visible || instance.mirror.drawn())
        .map(|instance| instance.bounds.transformed(&instance.transform))
}

/// Whether a planned skinned instance casts into a spot light's shadow slot:
/// its model's conservative LOCAL-space pose envelope, transformed by the
/// instance's world matrix,
/// must intersect the slot's cone frustum. Pure CPU data logic (no GPU, no BVH —
/// entities are not in the world BVH), through the shared
/// `aabb_intersects_frustum`, so the caster cull provably agrees with the shadow
/// world reach's frustum test.
///
/// The renderer records only instances this returns `true` for into a given
/// slot's depth layer; an enemy whose transformed bound lies outside the cone is
/// not drawn into that slot. Drives the per-frame submitted-occluder counter that
/// verifies the "enemy outside the cone is not drawn" acceptance criterion.
pub fn instance_casts_into_cone(instance: &PlannedInstance, cone_planes: &[Vec4; 6]) -> bool {
    let world_bound = instance.bounds.transformed(&instance.transform);
    aabb_intersects_frustum(&world_bound, cone_planes)
}
