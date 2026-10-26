// Mesh frame planner: priority-ordered, group-atomic admission into the palette and instance budgets.
// See: context/lib/rendering_pipeline.md §9, §7.3a

use postretro_model::ModelHandle;
use postretro_render_data::cone_frustum::Aabb;

use super::frame_plan::{MeshFramePlan, MeshFramePlans, ModelDrawGroup, PlannedInstance};
use super::input::{MeshInstanceInput, MeshPresentation, MirrorReach};

/// Fixed per-frame bone-palette budget, in `BonePaletteEntry` slots (one slot =
/// one joint of one instance). Sized from a representative wave: ~64 concurrent
/// skinned instances at the real per-model joint count (well under `MAX_JOINTS =
/// 256` — rigged monsters here run a few dozen joints). 64 instances × 64 joints
/// = 4096 slots. At 64 B per `BonePaletteEntry` that is 256 KiB of VRAM for the
/// shared palette buffer — negligible against the engine's atlas/geometry
/// budgets. Instances whose palette run would exceed this are dropped (see
/// [`plan_mesh_frame`]); the cap is a soft visual limit, never a panic.
pub const MAX_PALETTE_ENTRIES: usize = 4096;

/// Fixed per-frame instance budget — the cap on how many instances the per-frame
/// instance SSBO can hold. Defined here (the GPU-free planning half); the renderer
/// (`mesh_pass.rs`) imports this const and sizes that SSBO to exactly this value,
/// so the planner MUST drop instances past it or the GPU
/// layer's `write_buffer` runs off the end of the buffer and wgpu validation
/// panics. This is a SEPARATE cap from the palette budget, and its real job is
/// bounding unbounded instance growth: without it, a flood of instances would grow
/// the instance count without limit. Every model consumes at least one palette
/// slot (rigid / static-prop models carry one identity joint, so `run == 1`), so —
/// with `MAX_INSTANCES == MAX_PALETTE_ENTRIES` — the palette cap fires no later than
/// the instance cap even for a pure-rigid flood. One cap value covers both buffers.
pub const MAX_INSTANCES: usize = MAX_PALETTE_ENTRIES;

/// The planner's last priority: world instances only the planar mirror
/// reaches, after forward draws (0), dynamic shadow casters (1) and
/// promoted-static relevance (2), and after every ghost.
const MIRROR_ONLY_PRIORITY: u8 = 3;

/// Per-model lookups the GPU-free frame planner needs from the renderer's model
/// cache: the skeleton's joint count (the palette-run length) and the model's
/// conservative local-space pose envelope (stamped onto each `PlannedInstance`).
/// `joint_count` returning `None` means the handle is not in the cache (never
/// uploaded). A missing holder skips its whole holder group; a missing
/// attachment is omitted while the cached holder remains drawable. Keeps the
/// planner GPU-free: the cache provides plain values, no wgpu reference crosses.
pub trait JointCounts {
    fn joint_count(&self, model: &ModelHandle) -> Option<u32>;
    /// The model's conservative local-space pose envelope, or a zero box if the
    /// handle is uncached (those instances are skipped before the bound is read).
    fn model_bounds(&self, model: &ModelHandle) -> Aabb;
}

/// Group the surviving instances by model and assign each a contiguous
/// bone-palette run, packing runs densely into the shared palette buffer.
///
/// Instances are bucketed by model handle in first-seen order (stable, cheap to
/// reason about — not sorted, since wave counts are small). Each instance gets a
/// run of `joint_count(model)` palette slots; runs are laid out back-to-back
/// across all instances of all groups. A holder and its contiguous attachment
/// inputs are reserved atomically. Their palette-cache identities carry one
/// shared holder id, so the planner rejects the whole group before assigning
/// any run. World and viewmodel membership is also part of atomic identity, so
/// the same raw holder id cannot merge both plans. A rejected group's members
/// are all counted in `dropped` when EITHER budget would overflow:
/// - its palette run would push the cursor past [`MAX_PALETTE_ENTRIES`] (a
///   partial run would corrupt skinning), or
/// - the running instance count would reach [`MAX_INSTANCES`] (the per-frame
///   instance SSBO is sized to that bound — a write past it panics wgpu).
///
/// Static / rigid `prop_mesh` models are not zero-joint: the loader gives them
/// a single identity joint, so `run == 1` and each instance still consumes one
/// palette slot — the palette cap can fire for them too, just at a much higher
/// instance count than skinned models. Missing or failed attachment models are
/// silently omitted and are not counted as budget drops; a missing holder still
/// suppresses its holder group.
///
/// The mesh collector may emit forward-visible, dynamic-shadow-only,
/// promoted-static-only and mirror-only instances. They are budgeted in that
/// order so broader static relevance cannot evict an explicitly opted dynamic
/// caster, neither shadow class can evict drawable meshes, and nothing only
/// the planar mirror reaches evicts a main-view admission.
///
/// The returned plan's groups carry dense instance offsets so the GPU layer can
/// write one flat instance SSBO and issue one instanced draw per group.
pub fn plan_mesh_frame(
    instances: &[MeshInstanceInput],
    joints: &impl JointCounts,
) -> MeshFramePlan {
    let mut palette_cursor = 0;
    let mut instance_cursor = 0;
    let mut plan = MeshFramePlan::default();
    for visibility_priority in 0..=MIRROR_ONLY_PRIORITY {
        plan_mesh_priority(
            instances,
            joints,
            &|_| true,
            visibility_priority,
            &mut palette_cursor,
            &mut instance_cursor,
            &mut plan,
        );
    }
    let mut offset_cursor = 0;
    assign_dense_instance_offsets(&mut plan, &mut offset_cursor);
    plan
}

/// Partition world and first-person viewmodel instances, plus the frame's ghost
/// instances, into plans that share one dense palette/instance allocation. The
/// structural split means callers pass only [`MeshFramePlans::world`] into
/// shadow-depth recording; no depth filter has to recognize a viewmodel or
/// ghost group. `ghosts` holds only [`MeshPresentation::Ghost`] instances, and
/// `instances` none.
pub fn plan_mesh_frame_plans(
    instances: &[MeshInstanceInput],
    ghosts: &[MeshInstanceInput],
    joints: &impl JointCounts,
) -> MeshFramePlans {
    debug_assert!(
        ghosts.iter().all(|ghost| ghost.presentation.is_ghost())
            && !instances
                .iter()
                .any(|instance| instance.presentation.is_ghost()),
        "ghost instances plan from their own list"
    );
    let mut palette_cursor = 0;
    let mut instance_cursor = 0;
    let mut world = MeshFramePlan::default();
    let mut viewmodel = MeshFramePlan::default();
    let mut ghost = MeshFramePlan::default();

    // The collector appends its small viewmodel set after world instances. Find
    // that boundary once so each priority scan visits only its own plan. Keep a
    // filtered fallback because this public CPU contract still accepts mixed
    // input order.
    let first_viewmodel = instances
        .iter()
        .position(|instance| instance.presentation.is_viewmodel());
    let viewmodels_are_a_suffix = first_viewmodel.is_none_or(|start| {
        instances[start..]
            .iter()
            .all(|instance| instance.presentation.is_viewmodel())
    });
    let world_split = viewmodels_are_a_suffix.then(|| first_viewmodel.unwrap_or(instances.len()));
    let plan_world = |visibility_priority: u8,
                      palette_cursor: &mut usize,
                      instance_cursor: &mut usize,
                      world: &mut MeshFramePlan| {
        match world_split {
            Some(split) => plan_mesh_priority(
                &instances[..split],
                joints,
                &|_| true,
                visibility_priority,
                palette_cursor,
                instance_cursor,
                world,
            ),
            None => plan_mesh_priority(
                instances,
                joints,
                &|instance| !instance.presentation.is_viewmodel(),
                visibility_priority,
                palette_cursor,
                instance_cursor,
                world,
            ),
        }
    };
    for visibility_priority in 0..MIRROR_ONLY_PRIORITY {
        plan_world(
            visibility_priority,
            &mut palette_cursor,
            &mut instance_cursor,
            &mut world,
        );
        match world_split {
            Some(split) => plan_mesh_priority(
                &instances[split..],
                joints,
                &|_| true,
                visibility_priority,
                &mut palette_cursor,
                &mut instance_cursor,
                &mut viewmodel,
            ),
            None => plan_mesh_priority(
                instances,
                joints,
                &|instance| instance.presentation.is_viewmodel(),
                visibility_priority,
                &mut palette_cursor,
                &mut instance_cursor,
                &mut viewmodel,
            ),
        }
    }

    // Ghosts plan after every real main-view group, shadow-only casters
    // included, so an overflow drops ghosts before any real mesh the main view
    // reaches.
    for visibility_priority in 0..MIRROR_ONLY_PRIORITY {
        plan_mesh_priority(
            ghosts,
            joints,
            &|_| true,
            visibility_priority,
            &mut palette_cursor,
            &mut instance_cursor,
            &mut ghost,
        );
    }

    // World instances only the planar mirror reaches plan last, after every
    // main-view admission and every ghost, so a full pool drops them first and
    // they never evict anything the main view draws or casts.
    // Only a mirror-only input reaches that priority, so the scan is skipped
    // when none exists; the cursor delta counts exactly what it admitted.
    let mut dropped_mirror_only = 0;
    let mut planned_mirror_only = 0;
    if instances.iter().any(|instance| instance.mirror.is_only()) {
        let dropped_before_mirror = world.dropped;
        let planned_before_mirror = instance_cursor;
        plan_world(
            MIRROR_ONLY_PRIORITY,
            &mut palette_cursor,
            &mut instance_cursor,
            &mut world,
        );
        dropped_mirror_only = world.dropped - dropped_before_mirror;
        planned_mirror_only = (instance_cursor - planned_before_mirror) as u32;
    }

    let mut offset_cursor = 0;
    assign_dense_instance_offsets(&mut world, &mut offset_cursor);
    assign_dense_instance_offsets(&mut viewmodel, &mut offset_cursor);
    assign_dense_instance_offsets(&mut ghost, &mut offset_cursor);
    MeshFramePlans {
        world,
        viewmodel,
        ghost,
        dropped_mirror_only,
        planned_mirror_only,
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_mesh_priority(
    instances: &[MeshInstanceInput],
    joints: &impl JointCounts,
    include: &impl Fn(&MeshInstanceInput) -> bool,
    visibility_priority: u8,
    palette_cursor: &mut usize,
    instance_cursor: &mut usize,
    plan: &mut MeshFramePlan,
) {
    // Budget forward-visible groups first, then dynamic-shadow casters, then the
    // broader promoted-static-only relevance set, then what only the planar
    // mirror reaches. Collector output keeps each holder immediately followed
    // by its attachments, so one scan per priority preserves group order
    // without an allocation.
    let mut group_start = 0;
    while group_start < instances.len() {
        let holder_group = instances[group_start].palette_cache_key.holder_group();
        let plan_kind = instances[group_start].presentation.plan();
        let mut group_end = group_start + 1;
        while group_end < instances.len()
            && instances[group_end].palette_cache_key.holder_group() == holder_group
            && instances[group_end].presentation.plan() == plan_kind
        {
            group_end += 1;
        }
        let input_group = &instances[group_start..group_end];
        if !include(&input_group[0]) {
            group_start = group_end;
            continue;
        }
        debug_assert!(
            input_group.iter().all(include),
            "a holder and its attachments must share one plan"
        );
        let forward_visible = input_group.iter().any(|instance| instance.forward_visible);
        let dynamic_shadow_visible = input_group
            .iter()
            .any(|instance| instance.dynamic_shadow_visible);
        // Only a world draw has a mirror reach; the mirror never draws a
        // viewmodel or a ghost.
        let world_draw = plan_kind == MeshPresentation::World.plan();
        let mirror = if !world_draw {
            MirrorReach::Hidden
        } else if input_group.iter().all(|instance| instance.mirror.is_only()) {
            MirrorReach::Only
        } else if input_group.iter().any(|instance| instance.mirror.drawn()) {
            MirrorReach::AlsoMain
        } else {
            MirrorReach::Hidden
        };
        debug_assert!(
            !mirror.is_only() || (!forward_visible && !dynamic_shadow_visible),
            "a mirror-only instance has no main-view reach"
        );
        let priority = if mirror.is_only() {
            MIRROR_ONLY_PRIORITY
        } else if forward_visible {
            0
        } else if dynamic_shadow_visible {
            1
        } else {
            2
        };
        if priority == visibility_priority {
            plan_instance_group(
                input_group,
                forward_visible,
                dynamic_shadow_visible,
                mirror,
                joints,
                &mut plan.groups,
                palette_cursor,
                instance_cursor,
                &mut plan.dropped,
            );
        }
        group_start = group_end;
    }
}

fn assign_dense_instance_offsets(plan: &mut MeshFramePlan, instance_cursor: &mut u32) {
    let plan_instance_start = *instance_cursor;
    // Assign dense instance offsets in group order so the flat SSBO is filled
    // group-by-group; each group draws `instance_offset..+len`.
    for group in &mut plan.groups {
        group.instance_offset = *instance_cursor;
        *instance_cursor += group.instances.len() as u32;
    }
    plan.instance_count = *instance_cursor - plan_instance_start;
}

#[allow(clippy::too_many_arguments)]
fn plan_instance_group(
    input_group: &[MeshInstanceInput],
    forward_visible: bool,
    dynamic_shadow_visible: bool,
    mirror: MirrorReach,
    joints: &impl JointCounts,
    groups: &mut Vec<ModelDrawGroup>,
    palette_cursor: &mut usize,
    planned_instance_count: &mut usize,
    dropped: &mut u32,
) {
    let Some(holder_joints) = input_group
        .first()
        .and_then(|instance| joints.joint_count(&instance.model))
    else {
        // A missing holder model suppresses the whole visual group. Cache
        // absence is not a budget drop, and an unavailable attachment is
        // omitted below so the holder can still render.
        return;
    };

    let mut palette_entries = holder_joints as usize;
    let mut available_instances = 1usize;
    for instance in &input_group[1..] {
        let Some(joint_count) = joints.joint_count(&instance.model) else {
            continue;
        };
        let Some(next) = palette_entries.checked_add(joint_count as usize) else {
            *dropped = (*dropped).saturating_add(available_instances as u32);
            return;
        };
        palette_entries = next;
        available_instances += 1;
    }

    let instance_budget_overflows = available_instances > MAX_INSTANCES - *planned_instance_count;
    let palette_budget_overflows = palette_entries > MAX_PALETTE_ENTRIES - *palette_cursor;
    if instance_budget_overflows || palette_budget_overflows {
        *dropped = (*dropped).saturating_add(available_instances as u32);
        return;
    }

    for (index, instance) in input_group.iter().enumerate() {
        let Some(run) = joints
            .joint_count(&instance.model)
            .map(|count| count as usize)
        else {
            debug_assert!(index > 0, "holder cache was preflighted");
            continue;
        };
        let palette_base = *palette_cursor as u32;
        *palette_cursor += run;
        *planned_instance_count += 1;

        let planned = PlannedInstance {
            transform: instance.transform,
            shadow_bias_scale: instance.shadow_bias_scale,
            palette_base,
            phase_seed: instance.phase_seed,
            palette_cache_key: instance.palette_cache_key,
            bounds: joints.model_bounds(&instance.model),
            sample: instance.sample,
            pose_inputs: instance.pose_inputs,
            capture: instance.capture,
            resample: instance.resample,
            // Holder visibility is a group contract. Normalize every member so
            // forward and shadow draws cannot split malformed mixed inputs.
            forward_visible,
            dynamic_shadow_visible,
            mirror,
            ghost_look: match instance.presentation {
                MeshPresentation::Ghost(look) => Some(look),
                MeshPresentation::World | MeshPresentation::Viewmodel => None,
            },
        };

        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.model == instance.model)
        {
            group.instances.push(planned);
        } else {
            groups.push(ModelDrawGroup {
                model: instance.model.clone(),
                instance_offset: 0,
                instances: vec![planned],
            });
        }
    }
}
