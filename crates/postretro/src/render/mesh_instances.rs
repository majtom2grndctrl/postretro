// Skinned-mesh per-frame draw planning: group instances by model, assign each a
// contiguous bone-palette run, and drop overflow past either fixed budget
// (palette slots or the per-frame instance count).
// See: context/lib/rendering_pipeline.md §9
//
// GPU-free by contract — this is the data-logic half of the renderer's mesh
// pass (development_guide §4.1). The renderer's thin GPU layer in `mesh_pass.rs`
// consumes a [`MeshFramePlan`] to write the palette/instance SSBOs and record
// the instanced draws. Pure functions here so grouping, base-index assignment,
// and overflow are unit-testable without a GPU.

use glam::{Mat4, Vec4};

use crate::lighting::cone_frustum::{Aabb, aabb_intersects_frustum};
use crate::model::ModelHandle;
// The per-instance sample-parameter types (`ClipSample`, `MeshSampleParams`,
// `FadeSource`, `MeshFade`, `SnapshotTag`, `CaptureInstruction`) and the
// `instance_phase` per-instance phase helper are render-free plain data/logic
// and now live in `crate::model::sample_params`, imported directly by both the
// renderer (`mesh_pass`, `mesh_render`) and the game side (`mesh_anim`, the
// hit-zone facility) — no renderer dependency crosses into game code.
use crate::model::sample_params::{CaptureInstruction, MeshSampleParams};

/// Fixed per-frame bone-palette budget, in `BonePaletteEntry` slots (one slot =
/// one joint of one instance). Sized from a representative wave: ~64 concurrent
/// skinned instances at the real per-model joint count (well under `MAX_JOINTS =
/// 256` — rigged monsters here run a few dozen joints). 64 instances × 64 joints
/// = 4096 slots. At 64 B per `BonePaletteEntry` that is 256 KiB of VRAM for the
/// shared palette buffer — negligible against the engine's atlas/geometry
/// budgets. Instances whose palette run would exceed this are dropped (see
/// [`plan_mesh_frame`]); the cap is a soft visual limit, never a panic.
pub(crate) const MAX_PALETTE_ENTRIES: usize = 4096;

/// Fixed per-frame instance budget — the cap on how many instances the per-frame
/// instance SSBO can hold. Defined here (the GPU-free planning half); the renderer
/// (`mesh_pass.rs`) imports this const and sizes that SSBO to exactly this value,
/// so the planner MUST drop instances past it or the GPU
/// layer's `write_buffer` runs off the end of the buffer and wgpu validation
/// panics. This is a SEPARATE cap from the palette budget: a zero-joint (rigid /
/// static-prop) model consumes no palette slots, so the palette cap never fires
/// for it — without this instance cap, a flood of rigid props would grow the
/// instance count unbounded. Equal to `MAX_PALETTE_ENTRIES` because each instance
/// consumes at least one palette slot in the skinned case, so one cap value
/// covers both buffers.
pub(crate) const MAX_INSTANCES: usize = MAX_PALETTE_ENTRIES;

/// One skinned-mesh instance to consider for this frame: which model it draws,
/// its final interpolated world transform, a deterministic phase seed (the raw
/// `EntityId`) used to de-sync animation across a wave, the resolved per-frame
/// sample parameters, and an optional one-time capture instruction. Produced by
/// the render-frame collector (game side) after the visibility cull; consumed by
/// the frame planner below.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MeshInstanceInput {
    pub(crate) model: ModelHandle,
    pub(crate) transform: Mat4,
    /// Deterministic per-instance animation-phase seed (raw `EntityId`). Folded
    /// into a phase offset so a spawned wave does not animate lock-step, and the
    /// key into the snapshot store.
    pub(crate) phase_seed: u32,
    /// Resolved sample parameters: primary clip leg + optional crossfade. The
    /// collector computes these from entity state + the clip table; for a
    /// stateless `prop_mesh` entity this is [`MeshSampleParams::stateless`].
    pub(crate) sample: MeshSampleParams,
    /// One-time `"smooth"`-interrupt snapshot-capture instruction for this frame,
    /// if the entity crossed an interrupt this frame. Evaluated by the pass into
    /// the per-entity snapshot store before sampling (idempotent by tag).
    pub(crate) capture: Option<CaptureInstruction>,
    /// Animation time-slicing decision: `true` → re-sample this
    /// instance's pose this frame; `false` → the pass may re-upload its cached
    /// palette run and skip sampling. Decided game-side from the instance's
    /// camera distance bucket + frame-stride phase, forced `true` on a state
    /// change, an active crossfade, or a renderer-side cache miss (the pass
    /// upgrades a miss to a resample regardless of this flag). A `Copy` bool —
    /// no per-instance heap.
    pub(crate) resample: bool,
    /// Whether this instance is in the camera's portal PVS (the forward-visibility
    /// cull). `true` → drawn by both the FORWARD mesh pass and the shadow depth
    /// passes; `false` → an OFF-PVS shadow caster, drawn into shadow maps ONLY
    /// (its leaf left the camera PVS — e.g. pitching down — but it still sits in a
    /// dynamic shadow light's influence volume and must keep casting). The
    /// collector emits an off-PVS instance only when it is inside that light-volume
    /// union, so the off-screen pose cost stays bounded. `record_draws` skips
    /// `false` instances; the shadow passes draw the whole plan.
    ///
    /// Regression: entity shadow caster dropped when its leaf left the camera PVS
    /// (pitch-down) — the forward cull pre-removed the caster before the depth pass.
    pub(crate) forward_visible: bool,
}

/// One instance's resolved placement in the frame plan: its world transform, the
/// base index of its contiguous palette run in the shared buffer, its phase seed
/// (carried through so the GPU layer can sample its clip into the run at a
/// per-instance phase), and its model's LOCAL-space bound.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlannedInstance {
    pub(crate) transform: Mat4,
    pub(crate) palette_base: u32,
    pub(crate) phase_seed: u32,
    /// The instance's model's LOCAL-space AABB (bind-pose bound), stamped from
    /// the renderer's model cache at plan time. The per-light caster cull
    /// transforms this by `transform` and tests it against a light's
    /// cone/face frustum to decide whether the instance casts into that light's
    /// shadow map. Surfaced CPU-side here; the GPU draw never reads it.
    pub(crate) bounds: Aabb,
    /// Resolved per-frame sample parameters carried verbatim from the collector
    /// — the GPU layer feeds these to the pose sampler (single / blended /
    /// snapshot-blended), replacing the hardcoded first-clip-at-render-clock path.
    pub(crate) sample: MeshSampleParams,
    /// One-time `"smooth"`-interrupt capture instruction for this frame, if any.
    /// The GPU layer evaluates it into the snapshot store (idempotent by tag)
    /// before sampling this frame's pose.
    pub(crate) capture: Option<CaptureInstruction>,
    /// Animation time-slicing decision, carried verbatim from the
    /// instance input. `true` → the pass samples this instance's pose AND
    /// refreshes its palette cache; `false` → the pass re-uploads the cached
    /// palette run with no sampling. A renderer-side cache MISS upgrades a
    /// `false` to a resample regardless (the collector cannot see cache state),
    /// so a culled instance re-entering view never shows a stale pose. `Copy`
    /// bool — no per-instance heap.
    pub(crate) resample: bool,
    /// Carried verbatim from [`MeshInstanceInput::forward_visible`]. `true` → in
    /// the camera PVS, drawn by the forward mesh pass; `false` → an off-PVS shadow
    /// caster, drawn into shadow maps only. The combined plan poses + budgets BOTH
    /// kinds together (shared posed buffers, one budget), so the shadow depth pass
    /// and the forward pass read the identical palette/instance buffers; the
    /// forward `record_draws` then filters out `false` instances.
    pub(crate) forward_visible: bool,
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
/// dropped because either budget was exhausted (palette slots or the per-frame
/// instance cap).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct MeshFramePlan {
    pub(crate) groups: Vec<ModelDrawGroup>,
    /// Total planned instances across all groups (== sum of group lengths). The
    /// instance SSBO is filled densely in group order, so a group's instances
    /// occupy `instance_offset..instance_offset + len`.
    pub(crate) instance_count: u32,
    /// Instances dropped because EITHER their palette run would exceed
    /// `MAX_PALETTE_ENTRIES` OR the instance count would reach `MAX_INSTANCES`
    /// (the per-frame instance SSBO size — the only cap that fires for zero-joint
    /// rigid props). The caller rate-limits a warning when this is non-zero.
    pub(crate) dropped: u32,
}

/// Per-model lookups the GPU-free frame planner needs from the renderer's model
/// cache: the skeleton's joint count (the palette-run length) and the model's
/// local-space bound (stamped onto each `PlannedInstance` for the caster cull).
/// `joint_count` returning `None` means the handle is not in the cache (never
/// uploaded) — its instances are skipped, not budget-dropped. Keeps the planner
/// GPU-free: the cache provides plain values, no wgpu reference crosses.
pub(crate) trait JointCounts {
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
/// across all instances of all groups. An instance is DROPPED (counted in
/// `dropped`) rather than truncated when EITHER budget would overflow:
/// - its palette run would push the cursor past [`MAX_PALETTE_ENTRIES`] (a
///   partial run would corrupt skinning), or
/// - the running instance count would reach [`MAX_INSTANCES`] (the per-frame
///   instance SSBO is sized to that bound — a write past it panics wgpu).
///
/// The instance cap is the only one that fires for zero-joint (rigid / static
/// `prop_mesh`) models, since they consume no palette slots. An instance whose
/// model is absent from `joints` (never uploaded) is silently skipped and not
/// counted as a budget drop.
///
/// COMBINED budget over forward + shadow casters: `instances` carries BOTH the
/// camera-PVS-visible instances (`forward_visible == true`) AND the off-PVS
/// shadow casters (`forward_visible == false`) the collector emitted. They share
/// one plan — one posed palette/instance buffer, one budget — so an off-screen
/// caster draws down the SAME `MAX_INSTANCES`/palette pool as the on-screen
/// forward set (off-PVS casters can never silently evict on-screen forward
/// instances by being budgeted separately). The shadow depth passes draw the
/// whole plan; the forward pass filters to `forward_visible` instances.
///
/// The returned plan's groups carry dense instance offsets so the GPU layer can
/// write one flat instance SSBO and issue one instanced draw per group.
pub(crate) fn plan_mesh_frame(
    instances: &[MeshInstanceInput],
    joints: &impl JointCounts,
) -> MeshFramePlan {
    let mut groups: Vec<ModelDrawGroup> = Vec::new();
    let mut palette_cursor: usize = 0;
    let mut instance_count: usize = 0;
    let mut dropped: u32 = 0;

    // Budget the FORWARD-visible (camera-PVS) set first, then the OFF-PVS shadow
    // casters, so an off-screen caster can only consume budget the on-screen set
    // left behind — it can never evict a forward instance under contention
    // (reviewer guard: combined budget must not let off-screen casters silently
    // drop on-screen draws). Two-pass `filter` over the input slice, no clone.
    let forward_first = instances
        .iter()
        .filter(|i| i.forward_visible)
        .chain(instances.iter().filter(|i| !i.forward_visible));

    for inst in forward_first {
        let Some(joint_count) = joints.joint_count(&inst.model) else {
            // Model not in the cache (never uploaded) — skip, not a budget drop.
            continue;
        };
        let run = joint_count as usize;

        // Drop the instance if it would overflow EITHER budget. The instance cap
        // is what catches rigid / zero-joint props: their `run == 0` never trips
        // the palette cap, so without this check the instance count — and the
        // GPU layer's per-instance SSBO writes — would run unbounded past the
        // buffer the renderer sized to `MAX_INSTANCES` and panic wgpu.
        if instance_count >= MAX_INSTANCES || palette_cursor + run > MAX_PALETTE_ENTRIES {
            dropped += 1;
            continue;
        }
        let palette_base = palette_cursor as u32;
        palette_cursor += run;
        instance_count += 1;

        let planned = PlannedInstance {
            transform: inst.transform,
            palette_base,
            phase_seed: inst.phase_seed,
            bounds: joints.model_bounds(&inst.model),
            sample: inst.sample,
            capture: inst.capture,
            resample: inst.resample,
            forward_visible: inst.forward_visible,
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
pub(crate) fn instance_casts_into_cone(
    instance: &PlannedInstance,
    cone_planes: &[Vec4; 6],
) -> bool {
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
            phase_seed: seed,
            sample: MeshSampleParams::stateless(0.0),
            capture: None,
            resample: true,
            forward_visible: true,
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
        let total: usize = plan.groups.iter().map(|g| g.instances.len()).sum();
        assert_eq!(total, MAX_INSTANCES, "surviving instances match the count");
    }

    /// An off-PVS shadow caster (`forward_visible == false`) for the same model.
    fn shadow_caster(model: &str, x: f32, seed: u32) -> MeshInstanceInput {
        let mut i = instance(model, x, seed);
        i.forward_visible = false;
        i
    }

    #[test]
    fn plan_budgets_forward_visible_before_off_pvs_casters() {
        // Reviewer guard: the COMBINED budget must not let off-screen shadow casters
        // evict on-screen forward instances. With a budget of exactly 2 instances
        // and the off-PVS caster listed FIRST in the input, both forward-visible
        // instances must still survive (the planner budgets the forward set first);
        // the off-PVS caster is the one dropped.
        let per = (MAX_PALETTE_ENTRIES / 2) as u32; // two runs fill the palette budget
        let joints = joints(&[("grunt", per)]);
        let instances = [
            shadow_caster("grunt", 99.0, 2), // off-PVS, listed first
            instance("grunt", 1.0, 0),       // forward-visible
            instance("grunt", 2.0, 1),       // forward-visible
        ];
        let plan = plan_mesh_frame(&instances, &joints);

        assert_eq!(plan.instance_count, 2, "two instances fit the budget");
        assert_eq!(plan.dropped, 1, "the off-PVS caster is dropped, not a forward one");
        // Both survivors are the forward-visible instances (seeds 0 and 1); the
        // off-PVS caster (seed 2) was evicted despite being listed first.
        let seeds: Vec<u32> = plan
            .groups
            .iter()
            .flat_map(|g| g.instances.iter().map(|i| i.phase_seed))
            .collect();
        assert!(
            seeds.contains(&0) && seeds.contains(&1) && !seeds.contains(&2),
            "forward-visible instances survive over the off-PVS caster: {seeds:?}",
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

    /// AC#2: the per-light caster cull keeps an instance whose transformed bound
    /// is inside the cone and drops one whose transformed bound is outside it.
    /// Pure CPU: builds the cone planes from a spotlight aimed down -Z, then
    /// places one instance inside the cone and one far off-axis. The LOCAL bound
    /// is identical for both — only the world transform moves it in/out, proving
    /// the transform-then-test path culls correctly.
    #[test]
    fn caster_cull_keeps_in_cone_drops_out_of_cone() {
        use crate::lighting::cone_frustum::cone_frustum_planes;
        use crate::lighting::spot_shadow::light_space_matrix;
        use crate::prl::{FalloffModel, LightType, MapLight, ShadowType};

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
            leaf_index: 0,
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
            palette_base: 0,
            phase_seed: 0,
            bounds: local,
            sample: MeshSampleParams::stateless(0.0),
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
            palette_base: 0,
            phase_seed: 0,
            bounds: local,
            sample: MeshSampleParams::stateless(0.0),
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
        use crate::lighting::cone_frustum::cone_frustum_planes;
        use crate::lighting::spot_shadow::light_space_matrix;
        use crate::prl::{FalloffModel, LightType, MapLight, ShadowType};

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
            leaf_index: 0,
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
            palette_base: 0,
            phase_seed: 0,
            bounds: bar,
            sample: MeshSampleParams::stateless(0.0),
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
