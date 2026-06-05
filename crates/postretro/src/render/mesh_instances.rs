// Skinned-mesh per-frame draw planning: group instances by model, assign each a
// contiguous bone-palette run, and drop overflow past the fixed palette budget.
// See: context/lib/rendering_pipeline.md §9
//
// GPU-free by contract — this is the data-logic half of the renderer's mesh
// pass (development_guide §4.1). The renderer's thin GPU layer in `mesh_pass.rs`
// consumes a [`MeshFramePlan`] to write the palette/instance SSBOs and record
// the instanced draws. Pure functions here so grouping, base-index assignment,
// and overflow are unit-testable without a GPU.

use glam::Mat4;

use crate::model::ModelHandle;

/// Fixed per-frame bone-palette budget, in `BonePaletteEntry` slots (one slot =
/// one joint of one instance). Sized from a representative wave: ~64 concurrent
/// skinned instances at the real per-model joint count (well under `MAX_JOINTS =
/// 256` — rigged monsters here run a few dozen joints). 64 instances × 64 joints
/// = 4096 slots. At 64 B per `BonePaletteEntry` that is 256 KiB of VRAM for the
/// shared palette buffer — negligible against the engine's atlas/geometry
/// budgets. Instances whose palette run would exceed this are dropped (see
/// [`plan_mesh_frame`]); the cap is a soft visual limit, never a panic.
pub(crate) const MAX_PALETTE_ENTRIES: usize = 4096;

/// One skinned-mesh instance to consider for this frame: which model it draws,
/// its final interpolated world transform, and a deterministic phase seed (the
/// raw `EntityId`) used to de-sync animation across a wave. Produced by the
/// render-frame collector (game side) after the visibility cull; consumed by the
/// frame planner below.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MeshInstanceInput {
    pub(crate) model: ModelHandle,
    pub(crate) transform: Mat4,
    /// Deterministic per-instance animation-phase seed (raw `EntityId`). Folded
    /// into a phase offset so a spawned wave does not animate lock-step.
    pub(crate) phase_seed: u32,
}

/// One instance's resolved placement in the frame plan: its world transform, the
/// base index of its contiguous palette run in the shared buffer, and its phase
/// seed (carried through so the GPU layer can sample its clip into the run at a
/// per-instance phase).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlannedInstance {
    pub(crate) transform: Mat4,
    pub(crate) palette_base: u32,
    pub(crate) phase_seed: u32,
}

/// All instances of one model, batched for a single instanced `draw_indexed` per
/// submesh. The instances are contiguous in the per-frame instance SSBO, so the
/// draw uses `instance_offset..instance_offset + instances.len()` and the shader
/// reads each instance via `@builtin(instance_index)`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelDrawGroup {
    pub(crate) model: ModelHandle,
    /// Offset of this group's first instance in the flat instance SSBO.
    pub(crate) instance_offset: u32,
    pub(crate) instances: Vec<PlannedInstance>,
}

/// The per-frame skinned-mesh draw plan: one group per distinct model (in
/// first-seen order), the flat instance count, and how many instances were
/// dropped because the palette budget was exhausted.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct MeshFramePlan {
    pub(crate) groups: Vec<ModelDrawGroup>,
    /// Total planned instances across all groups (== sum of group lengths). The
    /// instance SSBO is filled densely in group order, so a group's instances
    /// occupy `instance_offset..instance_offset + len`.
    pub(crate) instance_count: u32,
    /// Instances dropped because their palette run would exceed the budget. The
    /// caller rate-limits a warning when this is non-zero.
    pub(crate) dropped: u32,
}

/// Look up a model's joint count by handle. The renderer's model cache provides
/// this (each `UploadedModel` knows its skeleton's joint count); the planner
/// only needs the count, keeping it GPU-free. Returns `None` for a handle that
/// is not in the cache — its instances are skipped (the model never uploaded).
pub(crate) trait JointCounts {
    fn joint_count(&self, model: &ModelHandle) -> Option<u32>;
}

/// Group the surviving instances by model and assign each a contiguous
/// bone-palette run, packing runs densely into the shared palette buffer.
///
/// Instances are bucketed by model handle in first-seen order (stable, cheap to
/// reason about — not sorted, since wave counts are small). Each instance gets a
/// run of `joint_count(model)` palette slots; runs are laid out back-to-back
/// across all instances of all groups. An instance whose run would push the
/// running palette cursor past [`MAX_PALETTE_ENTRIES`] is DROPPED (counted in
/// `dropped`) rather than truncated — a partial run would corrupt skinning. An
/// instance whose model is absent from `joints` (never uploaded) is silently
/// skipped and not counted as a budget drop.
///
/// The returned plan's groups carry dense instance offsets so the GPU layer can
/// write one flat instance SSBO and issue one instanced draw per group.
pub(crate) fn plan_mesh_frame(
    instances: &[MeshInstanceInput],
    joints: &impl JointCounts,
) -> MeshFramePlan {
    let mut groups: Vec<ModelDrawGroup> = Vec::new();
    let mut palette_cursor: usize = 0;
    let mut dropped: u32 = 0;

    for inst in instances {
        let Some(joint_count) = joints.joint_count(&inst.model) else {
            // Model not in the cache (never uploaded) — skip, not a budget drop.
            continue;
        };
        let run = joint_count as usize;

        // Drop the instance if its run does not fit the remaining budget. A
        // zero-joint model (degenerate) consumes no budget and always fits.
        if palette_cursor + run > MAX_PALETTE_ENTRIES {
            dropped += 1;
            continue;
        }
        let palette_base = palette_cursor as u32;
        palette_cursor += run;

        let planned = PlannedInstance {
            transform: inst.transform,
            palette_base,
            phase_seed: inst.phase_seed,
        };

        // Append to the existing group for this model, or start a new one.
        if let Some(group) = groups.iter_mut().find(|g| g.model == inst.model) {
            group.instances.push(planned);
        } else {
            groups.push(ModelDrawGroup {
                model: inst.model.clone(),
                instance_offset: 0, // assigned in the dense-offset pass below
                instances: vec![planned],
            });
        }
    }

    // Assign dense instance offsets in group order so the flat SSBO is filled
    // group-by-group; each group draws `instance_offset..+len`.
    let mut instance_offset: u32 = 0;
    for group in &mut groups {
        group.instance_offset = instance_offset;
        instance_offset += group.instances.len() as u32;
    }

    MeshFramePlan {
        groups,
        instance_count: instance_offset,
        dropped,
    }
}

/// Derive a per-instance animation phase offset (seconds) from the instance's
/// deterministic seed (raw `EntityId`). Spreads a spawned wave across the clip
/// so instances do not animate lock-step. The seed's low bits (entity slot
/// index) vary per entity, so hashing it and mapping to `[0, duration)` yields a
/// stable, well-distributed offset. A zero-length clip yields phase 0.
pub(crate) fn instance_phase(seed: u32, clip_duration: f32) -> f32 {
    if clip_duration <= 0.0 {
        return 0.0;
    }
    // Cheap integer hash (splitmix32-style finalizer) so adjacent seeds — which
    // EntityId slot indices tend to be — scatter rather than march in lockstep.
    let mut h = seed;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    // Map to [0, 1) then to [0, duration).
    let frac = (h as f32) / (u32::MAX as f32 + 1.0);
    frac * clip_duration
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use std::collections::HashMap;

    /// Test stand-in for the renderer's model cache: a fixed handle→joint-count
    /// map. Mirrors what `UploadedModel`'s skeleton length provides at runtime.
    struct FixedJoints(HashMap<String, u32>);

    impl JointCounts for FixedJoints {
        fn joint_count(&self, model: &ModelHandle) -> Option<u32> {
            self.0.get(model.as_str()).copied()
        }
    }

    fn joints(pairs: &[(&str, u32)]) -> FixedJoints {
        FixedJoints(pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect())
    }

    fn instance(model: &str, x: f32, seed: u32) -> MeshInstanceInput {
        MeshInstanceInput {
            model: ModelHandle::from(model),
            transform: Mat4::from_translation(Vec3::new(x, 0.0, 0.0)),
            phase_seed: seed,
        }
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
    fn plan_assigns_contiguous_non_overlapping_palette_runs() {
        // Two 10-joint instances → bases 0 and 10 (runs do not overlap).
        let joints = joints(&[("grunt", 10)]);
        let instances = [instance("grunt", 1.0, 0), instance("grunt", 2.0, 1)];
        let plan = plan_mesh_frame(&instances, &joints);

        let runs = &plan.groups[0].instances;
        assert_eq!(runs[0].palette_base, 0);
        assert_eq!(runs[1].palette_base, 10, "second run starts after the first");
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
    fn instance_phase_spreads_distinct_seeds_across_the_clip() {
        let duration = 2.0;
        let p0 = instance_phase(0, duration);
        let p1 = instance_phase(1, duration);
        let p2 = instance_phase(2, duration);
        // All in range.
        for p in [p0, p1, p2] {
            assert!((0.0..duration).contains(&p), "phase {p} in [0, {duration})");
        }
        // Adjacent seeds do not collapse to the same phase (de-sync the wave).
        assert!((p0 - p1).abs() > 1.0e-4, "seeds 0 and 1 produce distinct phases");
        assert!((p1 - p2).abs() > 1.0e-4, "seeds 1 and 2 produce distinct phases");
    }

    #[test]
    fn instance_phase_zero_for_zero_length_clip() {
        assert_eq!(instance_phase(12345, 0.0), 0.0);
    }
}
