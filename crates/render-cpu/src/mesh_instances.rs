// Skinned-mesh per-frame draw planning and overflow handling.
// See: context/lib/rendering_pipeline.md §9

use glam::{Mat4, Vec4};

use postretro_entities::PoseInputs;
use postretro_model::ModelHandle;
use postretro_render_data::cone_frustum::{Aabb, aabb_intersects_frustum};
// The per-instance sample-parameter types (`ClipSample`, `MeshSampleParams`,
// `FadeSource`, `MeshFade`, `SnapshotTag`, `CaptureInstruction`) and the
// `instance_phase` per-instance phase helper are render-free plain data/logic
// and now live in `postretro_model::sample_params`, imported directly by both the
// renderer (`mesh_pass`) and game/scripting systems (`mesh_render`, `mesh_anim`,
// the hit-zone facility) — no renderer dependency crosses into game code.
use postretro_model::sample_params::{CaptureInstruction, MeshSampleParams};

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
/// the instance count without limit. Every model now consumes at least one palette
/// slot (rigid / static-prop models carry one identity joint, so `run == 1`), so —
/// with `MAX_INSTANCES == MAX_PALETTE_ENTRIES` — the palette cap fires no later than
/// the instance cap even for a pure-rigid flood. One cap value covers both buffers.
pub const MAX_INSTANCES: usize = MAX_PALETTE_ENTRIES;

/// Stable identity for one instance's renderer-side palette cache entry.
///
/// An attachment has no animation/snapshot identity of its own, but it still
/// needs a cache slot distinct from every entity body. Keeping this separate
/// from `phase_seed` prevents an attached rigid prop from overwriting a
/// time-sliced entity palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MeshPaletteCacheKey {
    Entity(u32),
    Attachment {
        holder: u32,
        attachment_index: usize,
    },
}

impl MeshPaletteCacheKey {
    /// Stable holder group used for atomic frame-budget admission.
    fn holder_group(self) -> u32 {
        match self {
            Self::Entity(entity) => entity,
            Self::Attachment { holder, .. } => holder,
        }
    }
}

/// One skinned-mesh instance to consider for this frame: which model it draws,
/// its final interpolated world transform, a deterministic phase seed (the raw
/// `EntityId`) used to de-sync animation across a wave, the resolved per-frame
/// sample parameters, and an optional one-time capture instruction. Produced by
/// the render-frame collector (game side) after forward visibility and selected
/// static-light shadow relevance are classified; consumed by the frame planner
/// below.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshInstanceInput {
    pub model: ModelHandle,
    pub transform: Mat4,
    /// Per-model multiplier for the skinned receiver-side pool-shadow bias.
    pub shadow_bias_scale: f32,
    /// Deterministic per-instance animation-phase seed (raw `EntityId`). Folded
    /// into a phase offset so a spawned wave does not animate lock-step, and the
    /// key into the snapshot store.
    pub phase_seed: u32,
    /// Stable renderer-side palette-cache identity. This differs from
    /// [`phase_seed`](Self::phase_seed) for attachment props, which never own
    /// animation snapshots but must not alias their holder's cached palette.
    pub palette_cache_key: MeshPaletteCacheKey,
    /// Resolved pose selection: explicit rest pose, primary clip leg, or an
    /// optional crossfade. The collector computes this from entity state and
    /// the clip table.
    pub sample: MeshSampleParams,
    /// Same-tick presentation inputs for the model's pose-modifier stack.
    pub pose_inputs: Option<PoseInputs>,
    /// One-time `"smooth"`-interrupt snapshot-capture instruction for this frame,
    /// if the entity crossed an interrupt this frame. Evaluated by the pass into
    /// the per-entity snapshot store before sampling (idempotent by tag).
    pub capture: Option<CaptureInstruction>,
    /// Animation time-slicing decision: `true` → re-sample this
    /// instance's pose this frame; `false` → the pass may re-upload its cached
    /// palette run and skip sampling. Decided game-side from the instance's
    /// camera distance bucket + frame-stride phase, forced `true` on a state
    /// change, an active crossfade, or a renderer-side cache miss (the pass
    /// upgrades a miss to a resample regardless of this flag). A `Copy` bool —
    /// no per-instance heap.
    pub resample: bool,
    /// In the camera's portal-visible cell set. Selected static-light shadow
    /// casters may be retained with this `false`; the forward draw filters them
    /// out while shadow depth passes may still consume them.
    pub forward_visible: bool,
    /// First-person weapon presentation. Viewmodels are planned separately from
    /// world instances so they use the shared palette/instance buffers without
    /// ever entering world shadow-depth passes.
    pub is_viewmodel: bool,
}

/// One instance's resolved placement in the frame plan: its world transform, the
/// base index of its contiguous palette run in the shared buffer, its phase seed
/// (carried through so the GPU layer can sample its clip into the run at a
/// per-instance phase), and its model's LOCAL-space bound.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedInstance {
    pub transform: Mat4,
    /// Carried unchanged into the renderer's 80-byte skinned instance record.
    pub shadow_bias_scale: f32,
    pub palette_base: u32,
    pub phase_seed: u32,
    pub palette_cache_key: MeshPaletteCacheKey,
    /// The instance's model's LOCAL-space AABB (bind-pose bound), stamped from
    /// the renderer's model cache at plan time. The per-light caster cull
    /// transforms this by `transform` and tests it against a light's
    /// cone/face frustum to decide whether the instance casts into that light's
    /// shadow map. Surfaced CPU-side here; the GPU draw never reads it.
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
    /// filters this flag so selected static-light shadow casters outside the
    /// forward-visible set do not draw in the color pass.
    pub forward_visible: bool,
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

/// The two control-flow plans sharing this frame's palette and instance SSBOs.
///
/// World instances receive the first budget admission so gameplay presentation
/// cannot evict the world/shadow set. The viewmodel plan's palette bases and
/// instance offsets continue after the world plan; neither plan owns a separate
/// GPU buffer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshFramePlans {
    pub world: MeshFramePlan,
    pub viewmodel: MeshFramePlan,
}

/// Per-model lookups the GPU-free frame planner needs from the renderer's model
/// cache: the skeleton's joint count (the palette-run length) and the model's
/// local-space bound (stamped onto each `PlannedInstance` for the caster cull).
/// `joint_count` returning `None` means the handle is not in the cache (never
/// uploaded). A missing holder skips its whole holder group; a missing
/// attachment is omitted while the cached holder remains drawable. Keeps the
/// planner GPU-free: the cache provides plain values, no wgpu reference crosses.
pub trait JointCounts {
    fn joint_count(&self, model: &ModelHandle) -> Option<u32>;
    /// The model's local-space AABB, or a zero box if the handle is uncached
    /// (those instances are skipped before the bound is read, so the value is a
    /// harmless default).
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
/// any run. A rejected group's members are all counted in `dropped` when EITHER
/// budget would overflow:
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
/// The mesh collector may emit mixed forward/non-forward instances when a
/// non-forward mesh is relevant to a selected static-light shadow. The
/// forward-visible set is budgeted first so shadow-only instances cannot evict
/// drawable meshes.
///
/// The returned plan's groups carry dense instance offsets so the GPU layer can
/// write one flat instance SSBO and issue one instanced draw per group.
pub fn plan_mesh_frame(
    instances: &[MeshInstanceInput],
    joints: &impl JointCounts,
) -> MeshFramePlan {
    let mut palette_cursor = 0;
    let mut instance_cursor = 0;
    plan_grouped_mesh_frame(
        instances,
        joints,
        |_| true,
        &mut palette_cursor,
        &mut instance_cursor,
    )
}

/// Partition world and first-person viewmodel instances into plans that share
/// one dense palette/instance allocation. The structural split means callers
/// pass only [`MeshFramePlans::world`] into shadow-depth recording; no depth
/// filter has to recognize a viewmodel group.
pub fn plan_mesh_frame_plans(
    instances: &[MeshInstanceInput],
    joints: &impl JointCounts,
) -> MeshFramePlans {
    let mut palette_cursor = 0;
    let mut instance_cursor = 0;
    let world = plan_grouped_mesh_frame(
        instances,
        joints,
        |instance| !instance.is_viewmodel,
        &mut palette_cursor,
        &mut instance_cursor,
    );
    let viewmodel = plan_grouped_mesh_frame(
        instances,
        joints,
        |instance| instance.is_viewmodel,
        &mut palette_cursor,
        &mut instance_cursor,
    );
    MeshFramePlans { world, viewmodel }
}

fn plan_grouped_mesh_frame(
    instances: &[MeshInstanceInput],
    joints: &impl JointCounts,
    include: impl Fn(&MeshInstanceInput) -> bool,
    palette_cursor: &mut usize,
    instance_cursor: &mut usize,
) -> MeshFramePlan {
    let mut groups: Vec<ModelDrawGroup> = Vec::new();
    let plan_instance_start = *instance_cursor as u32;
    let mut dropped: u32 = 0;

    // Budget forward-visible groups first so shadow-only inputs never evict a
    // forward group. Collector output keeps each holder immediately followed by
    // its attachments, so two scans preserve group order without an allocation.
    for group_is_forward in [true, false] {
        let mut group_start = 0;
        while group_start < instances.len() {
            let holder_group = instances[group_start].palette_cache_key.holder_group();
            let mut group_end = group_start + 1;
            while group_end < instances.len()
                && instances[group_end].palette_cache_key.holder_group() == holder_group
            {
                group_end += 1;
            }
            let input_group = &instances[group_start..group_end];
            if !include(&input_group[0]) {
                group_start = group_end;
                continue;
            }
            debug_assert!(
                input_group.iter().all(&include),
                "a holder and its attachments must share the world/viewmodel plan"
            );
            let forward_visible = input_group.iter().any(|instance| instance.forward_visible);
            if forward_visible == group_is_forward {
                plan_instance_group(
                    input_group,
                    forward_visible,
                    joints,
                    &mut groups,
                    palette_cursor,
                    instance_cursor,
                    &mut dropped,
                );
            }
            group_start = group_end;
        }
    }

    // Assign dense instance offsets in group order so the flat SSBO is filled
    // group-by-group; each group draws `instance_offset..+len`.
    let mut instance_offset = plan_instance_start;
    for group in &mut groups {
        group.instance_offset = instance_offset;
        instance_offset += group.instances.len() as u32;
    }
    let instance_count = instance_offset - plan_instance_start;
    *instance_cursor = instance_offset as usize;

    MeshFramePlan {
        groups,
        instance_count,
        dropped,
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_instance_group(
    input_group: &[MeshInstanceInput],
    forward_visible: bool,
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

/// Whether a planned skinned instance casts into a spot light's shadow slot:
/// its model's LOCAL-space bound, transformed by the instance's world matrix,
/// must intersect the slot's cone frustum. Pure CPU data logic (no GPU, no BVH —
/// entities are not in the world BVH), mirroring the GPU cone-cull convention via
/// the shared `aabb_intersects_frustum`, so the caster cull provably agrees with
/// the world cull's frustum test.
///
/// The renderer records only instances this returns `true` for into a given
/// slot's depth layer; an enemy whose transformed bound lies outside the cone is
/// not drawn into that slot. Drives the per-frame submitted-occluder counter that
/// verifies the "enemy outside the cone is not drawn" acceptance criterion.
pub fn instance_casts_into_cone(instance: &PlannedInstance, cone_planes: &[Vec4; 6]) -> bool {
    let world_bound = instance.bounds.transformed(&instance.transform);
    aabb_intersects_frustum(&world_bound, cone_planes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use std::collections::HashMap;

    /// Test stand-in for the renderer's model cache: a fixed handle→joint-count
    /// map plus an optional handle→bounds map. Mirrors what `UploadedModel`'s
    /// skeleton length and `model_bounds` provide at runtime. Bounds default to a
    /// zero box for handles not in the bounds map (matching the runtime default
    /// for an uncached handle).
    struct FixedJoints {
        counts: HashMap<String, u32>,
        bounds: HashMap<String, Aabb>,
    }

    impl JointCounts for FixedJoints {
        fn joint_count(&self, model: &ModelHandle) -> Option<u32> {
            self.counts.get(model.as_str()).copied()
        }

        fn model_bounds(&self, model: &ModelHandle) -> Aabb {
            self.bounds.get(model.as_str()).copied().unwrap_or_default()
        }
    }

    fn joints(pairs: &[(&str, u32)]) -> FixedJoints {
        FixedJoints {
            counts: pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            bounds: HashMap::new(),
        }
    }

    fn instance(model: &str, x: f32, seed: u32) -> MeshInstanceInput {
        MeshInstanceInput {
            model: ModelHandle::from(model),
            transform: Mat4::from_translation(Vec3::new(x, 0.0, 0.0)),
            shadow_bias_scale: 1.0,
            phase_seed: seed,
            palette_cache_key: MeshPaletteCacheKey::Entity(seed),
            sample: MeshSampleParams::stateless(0.0),
            pose_inputs: None,
            capture: None,
            resample: true,
            forward_visible: true,
            is_viewmodel: false,
        }
    }

    fn attachment_instance(
        model: &str,
        x: f32,
        holder: u32,
        attachment_index: usize,
    ) -> MeshInstanceInput {
        let mut input = instance(model, x, holder);
        input.palette_cache_key = MeshPaletteCacheKey::Attachment {
            holder,
            attachment_index,
        };
        input
    }

    #[test]
    fn plan_groups_same_model_instances_into_one_group() {
        let joints = joints(&[("grunt", 10)]);
        let instances = [instance("grunt", 1.0, 0), instance("grunt", 2.0, 1)];
        let plan = plan_mesh_frame(&instances, &joints);

        assert_eq!(plan.groups.len(), 1, "same model → one group");
        assert_eq!(plan.groups[0].instances.len(), 2);
        assert_eq!(plan.instance_count, 2);
        assert_eq!(plan.dropped, 0);
        // Distinct transforms preserved per instance.
        assert_eq!(plan.groups[0].instances[0].transform.w_axis.x, 1.0);
        assert_eq!(plan.groups[0].instances[1].transform.w_axis.x, 2.0);
    }

    #[test]
    fn planner_carries_pose_inputs_verbatim() {
        let joints = joints(&[("grunt", 10)]);
        let expected = PoseInputs {
            aim_pitch: 0.25,
            aim_yaw: -0.5,
            heading_yaw: 1.0,
            ..Default::default()
        };
        let mut input = instance("grunt", 1.0, 7);
        input.pose_inputs = Some(expected);

        let full = plan_mesh_frame(std::slice::from_ref(&input), &joints);
        assert_eq!(full.groups[0].instances[0].pose_inputs, Some(expected));
    }

    #[test]
    fn plan_assigns_contiguous_non_overlapping_palette_runs() {
        // Two 10-joint instances → bases 0 and 10 (runs do not overlap).
        let joints = joints(&[("grunt", 10)]);
        let instances = [instance("grunt", 1.0, 0), instance("grunt", 2.0, 1)];
        let plan = plan_mesh_frame(&instances, &joints);

        let runs = &plan.groups[0].instances;
        assert_eq!(runs[0].palette_base, 0);
        assert_eq!(
            runs[1].palette_base, 10,
            "second run starts after the first"
        );
    }

    #[test]
    fn plan_separates_distinct_models_into_distinct_groups() {
        let joints = joints(&[("grunt", 8), ("drone", 12)]);
        let instances = [
            instance("grunt", 1.0, 0),
            instance("drone", 2.0, 1),
            instance("grunt", 3.0, 2),
        ];
        let plan = plan_mesh_frame(&instances, &joints);

        assert_eq!(plan.groups.len(), 2, "two distinct models → two groups");
        // First-seen order: grunt, then drone.
        assert_eq!(plan.groups[0].model.as_str(), "grunt");
        assert_eq!(plan.groups[0].instances.len(), 2);
        assert_eq!(plan.groups[1].model.as_str(), "drone");
        assert_eq!(plan.groups[1].instances.len(), 1);

        // Dense instance offsets: grunt occupies 0..2, drone 2..3.
        assert_eq!(plan.groups[0].instance_offset, 0);
        assert_eq!(plan.groups[1].instance_offset, 2);
        assert_eq!(plan.instance_count, 3);

        // Palette runs are contiguous across groups in append order:
        // grunt#0 @0 (8), drone#0 @8 (12), grunt#1 @20 (8).
        assert_eq!(plan.groups[0].instances[0].palette_base, 0);
        assert_eq!(plan.groups[1].instances[0].palette_base, 8);
        assert_eq!(plan.groups[0].instances[1].palette_base, 20);
    }

    #[test]
    fn plan_drops_instances_past_palette_budget() {
        // Joint count chosen so the third instance overflows the budget.
        let per = (MAX_PALETTE_ENTRIES / 2) as u32; // two fit exactly, third drops
        let joints = joints(&[("big", per)]);
        let instances = [
            instance("big", 1.0, 0),
            instance("big", 2.0, 1),
            instance("big", 3.0, 2),
        ];
        let plan = plan_mesh_frame(&instances, &joints);

        assert_eq!(plan.instance_count, 2, "only two instances fit the budget");
        assert_eq!(plan.dropped, 1, "the third is dropped");
        // The two survivors keep valid, non-corrupting runs.
        let runs = &plan.groups[0].instances;
        assert_eq!(runs[0].palette_base, 0);
        assert_eq!(runs[1].palette_base, per);
        // No run exceeds the budget.
        for r in runs {
            assert!((r.palette_base + per) as usize <= MAX_PALETTE_ENTRIES);
        }
    }

    #[test]
    fn plan_packs_holder_and_attachment_normally_as_one_budget_group() {
        let joints = joints(&[("holder", 3), ("prop", 1), ("other", 2)]);
        let instances = [
            instance("holder", 1.0, 7),
            attachment_instance("prop", 2.0, 7, 0),
            instance("other", 3.0, 8),
        ];

        let plan = plan_mesh_frame(&instances, &joints);

        assert_eq!(plan.instance_count, 3);
        assert_eq!(plan.dropped, 0);
        assert_eq!(plan.groups[0].model, ModelHandle::from("holder"));
        assert_eq!(plan.groups[0].instances[0].palette_base, 0);
        assert_eq!(plan.groups[1].model, ModelHandle::from("prop"));
        assert_eq!(plan.groups[1].instances[0].palette_base, 3);
        assert_eq!(plan.groups[2].model, ModelHandle::from("other"));
        assert_eq!(plan.groups[2].instances[0].palette_base, 4);
    }

    #[test]
    fn plan_rejects_attachment_when_holder_run_does_not_fit() {
        // Regression: independent budgeting dropped the large holder but kept
        // its one-joint prop, producing a floating attachment.
        let joints = joints(&[
            ("prefix", 1),
            ("holder", MAX_PALETTE_ENTRIES as u32),
            ("prop", 1),
        ]);
        let instances = [
            instance("prefix", 0.0, 1),
            instance("holder", 1.0, 7),
            attachment_instance("prop", 2.0, 7, 0),
        ];

        let plan = plan_mesh_frame(&instances, &joints);

        assert_eq!(plan.instance_count, 1, "only the independent prefix fits");
        assert_eq!(plan.dropped, 2, "holder and attachment reject together");
        assert_eq!(plan.groups.len(), 1);
        assert_eq!(plan.groups[0].model, ModelHandle::from("prefix"));
    }

    #[test]
    fn plan_rejects_holder_when_attachment_run_does_not_fit() {
        // Regression: independent budgeting admitted the holder and rejected
        // its prop at the palette boundary.
        let joints = joints(&[("holder", (MAX_PALETTE_ENTRIES - 1) as u32), ("prop", 2)]);
        let instances = [
            instance("holder", 1.0, 7),
            attachment_instance("prop", 2.0, 7, 0),
        ];

        let plan = plan_mesh_frame(&instances, &joints);

        assert_eq!(plan.instance_count, 0);
        assert_eq!(plan.dropped, 2, "holder and attachment reject together");
        assert!(plan.groups.is_empty());
    }

    #[test]
    fn plan_rejects_holder_group_atomically_at_instance_budget() {
        let joints = joints(&[("prefix", 0), ("holder", 0), ("prop", 0)]);
        let mut instances: Vec<MeshInstanceInput> = (0..MAX_INSTANCES - 1)
            .map(|index| instance("prefix", index as f32, index as u32))
            .collect();
        let holder = u32::MAX;
        instances.push(instance("holder", 1.0, holder));
        instances.push(attachment_instance("prop", 2.0, holder, 0));

        let plan = plan_mesh_frame(&instances, &joints);

        assert_eq!(plan.instance_count as usize, MAX_INSTANCES - 1);
        assert_eq!(plan.dropped, 2, "instance admission is group-atomic");
        assert!(
            plan.groups
                .iter()
                .all(|group| group.model.as_str() == "prefix"),
            "neither holder member enters the plan",
        );
    }

    #[test]
    fn plan_caps_zero_joint_instances_at_instance_budget() {
        // Regression: rigid / static props have ZERO joints, so the palette cap
        // never fires for them. Without the separate instance cap, the instance
        // count grows unbounded past the GPU instance SSBO (sized to
        // MAX_INSTANCES) and the renderer's per-instance write_buffer panics.
        let joints = joints(&[("prop", 0)]);
        let overflow = MAX_INSTANCES + 100;
        let instances: Vec<MeshInstanceInput> = (0..overflow)
            .map(|i| instance("prop", i as f32, i as u32))
            .collect();
        let plan = plan_mesh_frame(&instances, &joints);

        assert_eq!(
            plan.instance_count as usize, MAX_INSTANCES,
            "instance count is capped at the per-frame instance budget",
        );
        assert_eq!(
            plan.dropped as usize,
            overflow - MAX_INSTANCES,
            "every instance past the cap is counted as dropped",
        );
        // Zero-joint runs consume no palette slots, so every survivor shares base 0.
        // Synthetic guard for the hypothetical 0-joint model (Skeleton::new still
        // permits an empty joint vec); not the real static-prop path, which now
        // carries one identity joint.
        let total: usize = plan.groups.iter().map(|g| g.instances.len()).sum();
        assert_eq!(total, MAX_INSTANCES, "surviving instances match the count");
    }

    /// A synthetic non-forward-visible instance for planner guard tests.
    fn non_forward_instance(model: &str, x: f32, seed: u32) -> MeshInstanceInput {
        let mut i = instance(model, x, seed);
        i.forward_visible = false;
        i
    }

    #[test]
    fn plan_budgets_forward_visible_before_non_forward_inputs() {
        // Defensive guard for mixed synthetic input. Budget of 2, non-forward
        // instance listed first — both forward instances survive.
        let per = (MAX_PALETTE_ENTRIES / 2) as u32; // two runs fill the palette budget
        let joints = joints(&[("grunt", per)]);
        let instances = [
            non_forward_instance("grunt", 99.0, 2), // non-forward, listed first
            instance("grunt", 1.0, 0),              // forward-visible
            instance("grunt", 2.0, 1),              // forward-visible
        ];
        let plan = plan_mesh_frame(&instances, &joints);

        assert_eq!(plan.instance_count, 2, "two instances fit the budget");
        assert_eq!(
            plan.dropped, 1,
            "the non-forward instance is dropped, not a forward one"
        );
        // Survivors are seeds 0 and 1 (forward); seed 2 (non-forward) was evicted.
        let seeds: Vec<u32> = plan
            .groups
            .iter()
            .flat_map(|g| g.instances.iter().map(|i| i.phase_seed))
            .collect();
        assert!(
            seeds.contains(&0) && seeds.contains(&1) && !seeds.contains(&2),
            "forward-visible instances survive over the non-forward input: {seeds:?}",
        );
        assert!(
            plan.groups
                .iter()
                .flat_map(|g| &g.instances)
                .all(|i| i.forward_visible),
            "only forward-visible instances survived the budget squeeze",
        );
    }

    #[test]
    fn plan_skips_uncached_model_without_counting_as_dropped() {
        // "ghost" is not in the joint map (never uploaded) → skipped, not dropped.
        let joints = joints(&[("grunt", 10)]);
        let instances = [instance("ghost", 1.0, 0), instance("grunt", 2.0, 1)];
        let plan = plan_mesh_frame(&instances, &joints);

        assert_eq!(plan.instance_count, 1, "only the cached model is planned");
        assert_eq!(plan.dropped, 0, "an uncached model is not a budget drop");
        assert_eq!(plan.groups.len(), 1);
        assert_eq!(plan.groups[0].model.as_str(), "grunt");
    }

    #[test]
    fn plans_partition_world_and_viewmodel_with_shared_buffer_offsets() {
        let joints = joints(&[("world", 3), ("viewmodel", 4)]);
        let mut viewmodel = instance("viewmodel", 0.0, 2);
        viewmodel.is_viewmodel = true;
        let instances = [
            instance("world", 1.0, 0),
            instance("world", 2.0, 1),
            viewmodel,
        ];

        let plans = plan_mesh_frame_plans(&instances, &joints);

        assert_eq!(plans.world.instance_count, 2);
        assert_eq!(plans.viewmodel.instance_count, 1);
        assert_eq!(plans.world.groups[0].model.as_str(), "world");
        assert_eq!(plans.viewmodel.groups[0].model.as_str(), "viewmodel");

        let world_group = &plans.world.groups[0];
        let viewmodel_group = &plans.viewmodel.groups[0];
        assert_eq!(world_group.instance_offset, 0);
        assert_eq!(viewmodel_group.instance_offset, 2);
        assert_eq!(world_group.instances[0].palette_base, 0);
        assert_eq!(world_group.instances[1].palette_base, 3);
        assert_eq!(viewmodel_group.instances[0].palette_base, 6);
    }

    #[test]
    fn missing_viewmodel_asset_leaves_no_viewmodel_plan_to_draw() {
        let joints = joints(&[("world", 2)]);
        let mut missing_viewmodel = instance("missing-viewmodel", 0.0, 2);
        missing_viewmodel.is_viewmodel = true;

        let plans = plan_mesh_frame_plans(&[instance("world", 1.0, 1), missing_viewmodel], &joints);

        assert_eq!(plans.world.instance_count, 1);
        assert!(plans.viewmodel.groups.is_empty());
        assert_eq!(plans.viewmodel.dropped, 0);
    }

    #[test]
    fn plan_keeps_cached_holder_when_attachment_model_is_uncached() {
        let joints = joints(&[("holder", 10)]);
        let instances = [
            instance("holder", 1.0, 7),
            attachment_instance("missing-prop", 2.0, 7, 0),
        ];

        let plan = plan_mesh_frame(&instances, &joints);

        assert_eq!(plan.instance_count, 1);
        assert_eq!(plan.dropped, 0, "cache absence is not budget pressure");
        assert_eq!(plan.groups.len(), 1);
        assert_eq!(plan.groups[0].model.as_str(), "holder");
    }

    /// AC#2: the per-light caster cull keeps an instance whose transformed bound
    /// is inside the cone and drops one whose transformed bound is outside it.
    /// Pure CPU: builds the cone planes from a spotlight aimed down -Z, then
    /// places one instance inside the cone and one far off-axis. The LOCAL bound
    /// is identical for both — only the world transform moves it in/out, proving
    /// the transform-then-test path culls correctly.
    #[test]
    fn caster_cull_keeps_in_cone_drops_out_of_cone() {
        use postretro_level_loader::{FalloffModel, LightType, MapLight, ShadowType};
        use postretro_lighting::light_space_matrix;
        use postretro_render_data::cone_frustum::cone_frustum_planes;

        // Spotlight at the origin aimed down -Z, 20 m range — same cone the
        // cone_frustum tests use.
        let light = MapLight {
            origin: [0.0, 0.0, 0.0],
            light_type: LightType::Spot,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 20.0,
            cone_angle_inner: 0.3,
            cone_angle_outer: 0.4,
            cone_direction: [0.0, 0.0, -1.0],
            is_dynamic: true,
            casts_entity_shadows: true,
            animated_slot: None,
            tags: vec![],
            cell_index: 0,
            shadow_type: ShadowType::StaticLightMap,
        };
        let planes = cone_frustum_planes(&light_space_matrix(&light));

        // A unit-ish local bound (1 m half-extents), like a rigged enemy.
        let local = Aabb {
            min: Vec3::new(-0.5, -0.5, -0.5),
            max: Vec3::new(0.5, 0.5, 0.5),
        };

        // Inside: 10 m down the cone axis.
        let inside = PlannedInstance {
            transform: Mat4::from_translation(Vec3::new(0.0, 0.0, -10.0)),
            shadow_bias_scale: 1.0,
            palette_base: 0,
            phase_seed: 0,
            palette_cache_key: MeshPaletteCacheKey::Entity(0),
            bounds: local,
            sample: MeshSampleParams::stateless(0.0),
            pose_inputs: None,
            capture: None,
            resample: true,
            forward_visible: true,
        };
        assert!(
            instance_casts_into_cone(&inside, &planes),
            "instance inside the cone must cast into the slot"
        );

        // Outside: far off-axis (+50 m in X) at the same depth — well beyond the
        // cone's angular spread.
        let outside = PlannedInstance {
            transform: Mat4::from_translation(Vec3::new(50.0, 0.0, -10.0)),
            shadow_bias_scale: 1.0,
            palette_base: 0,
            phase_seed: 0,
            palette_cache_key: MeshPaletteCacheKey::Entity(0),
            bounds: local,
            sample: MeshSampleParams::stateless(0.0),
            pose_inputs: None,
            capture: None,
            resample: true,
            forward_visible: true,
        };
        assert!(
            !instance_casts_into_cone(&outside, &planes),
            "instance outside the cone must not cast into the slot"
        );
    }

    /// A rotation that swings a long, thin local bound into the cone must be
    /// enclosed correctly — the transformed-corner method (not a component-wise
    /// min/max transform) is what makes the rotated box's true extent the test
    /// input. A bar pointing along local +X, rotated to point down -Z and placed
    /// on the cone axis, must classify as casting.
    #[test]
    fn caster_cull_encloses_rotated_bound() {
        use postretro_level_loader::{FalloffModel, LightType, MapLight, ShadowType};
        use postretro_lighting::light_space_matrix;
        use postretro_render_data::cone_frustum::cone_frustum_planes;

        let light = MapLight {
            origin: [0.0, 0.0, 0.0],
            light_type: LightType::Spot,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range: 20.0,
            cone_angle_inner: 0.3,
            cone_angle_outer: 0.4,
            cone_direction: [0.0, 0.0, -1.0],
            is_dynamic: true,
            casts_entity_shadows: true,
            animated_slot: None,
            tags: vec![],
            cell_index: 0,
            shadow_type: ShadowType::StaticLightMap,
        };
        let planes = cone_frustum_planes(&light_space_matrix(&light));

        // Long bar along local X, thin in Y/Z.
        let bar = Aabb {
            min: Vec3::new(-4.0, -0.1, -0.1),
            max: Vec3::new(4.0, 0.1, 0.1),
        };
        // Rotate -90° about Y so local +X points to world -Z, then drop it onto
        // the axis 10 m down the cone.
        let transform = Mat4::from_translation(Vec3::new(0.0, 0.0, -10.0))
            * Mat4::from_rotation_y(-std::f32::consts::FRAC_PI_2);
        let inst = PlannedInstance {
            transform,
            shadow_bias_scale: 1.0,
            palette_base: 0,
            phase_seed: 0,
            palette_cache_key: MeshPaletteCacheKey::Entity(0),
            bounds: bar,
            sample: MeshSampleParams::stateless(0.0),
            pose_inputs: None,
            capture: None,
            resample: true,
            forward_visible: true,
        };
        assert!(
            instance_casts_into_cone(&inst, &planes),
            "rotated bar on the cone axis must enclose correctly and cast"
        );
    }

    #[test]
    fn plan_stamps_model_local_bounds_onto_planned_instances() {
        // Each planned instance must carry its model's LOCAL-space bound (the
        // per-light caster cull transforms it by `transform` at cull time). The
        // planner stamps it from the model-info lookup, so two distinct models'
        // instances carry distinct bounds.
        let model_bounds = Aabb {
            min: Vec3::new(-1.0, -2.0, -3.0),
            max: Vec3::new(1.0, 2.0, 3.0),
        };
        let mut fixed = joints(&[("grunt", 8), ("drone", 4)]);
        fixed.bounds.insert("grunt".to_string(), model_bounds);
        // "drone" intentionally has NO bounds entry → defaults to the zero box.

        let instances = [instance("grunt", 1.0, 0), instance("drone", 2.0, 1)];
        let plan = plan_mesh_frame(&instances, &fixed);

        let grunt = &plan.groups[0];
        assert_eq!(grunt.model.as_str(), "grunt");
        assert_eq!(
            grunt.instances[0].bounds, model_bounds,
            "grunt instance carries its model's local bound"
        );

        let drone = &plan.groups[1];
        assert_eq!(drone.model.as_str(), "drone");
        assert_eq!(
            drone.instances[0].bounds,
            Aabb::default(),
            "a model with no bound entry defaults to the zero box"
        );
    }
}
