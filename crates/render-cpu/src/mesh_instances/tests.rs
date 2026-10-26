// Mesh frame planner tests: grouping, budgets, priority order, mirror reach.
// See: context/lib/testing_guide.md

use super::*;
use glam::{Mat4, Vec3};
use postretro_entities::PoseInputs;
use postretro_model::ModelHandle;
use postretro_model::sample_params::MeshSampleParams;
use postretro_render_data::cone_frustum::Aabb;
use std::collections::HashMap;

use crate::ghost_instance::GhostInstanceLook;

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
        dynamic_shadow_visible: true,
        mirror: MirrorReach::Hidden,
        presentation: MeshPresentation::World,
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

// Regression: candidate bounds were added to the SH compose gate before
// the mesh-frame budget dropped the corresponding instance.
#[test]
fn planned_forward_sample_bounds_exclude_palette_overflow() {
    let per = (MAX_PALETTE_ENTRIES / 2) as u32;
    let mut fixed = joints(&[("big", per)]);
    fixed.bounds.insert(
        "big".to_string(),
        Aabb {
            min: Vec3::splat(-0.5),
            max: Vec3::splat(0.5),
        },
    );
    let inputs = [
        instance("big", 1.0, 1),
        instance("big", 2.0, 2),
        instance("big", 99.0, 3),
    ];

    let plans = plan_mesh_frame_plans(&inputs, &[], &fixed);
    let bounds: Vec<_> = planned_forward_sample_bounds(&plans, true).collect();

    assert_eq!(plans.world.dropped, 1);
    assert_eq!(bounds.len(), 2);
    assert_eq!(bounds[0].min.x, 0.5);
    assert_eq!(bounds[1].min.x, 1.5);
    assert!(
        bounds.iter().all(|bounds| bounds.min.x < 90.0),
        "the overflow instance must not remain in the SH gate input",
    );
}

#[test]
fn planned_forward_sample_bounds_omit_viewmodel_for_offscreen_scene() {
    let fixed = joints(&[("world", 1), ("weapon", 1)]);
    let world = instance("world", 1.0, 1);
    let mut viewmodel = instance("weapon", 2.0, 2);
    viewmodel.presentation = MeshPresentation::Viewmodel;
    viewmodel.dynamic_shadow_visible = false;
    let plans = plan_mesh_frame_plans(&[world, viewmodel], &[], &fixed);

    assert_eq!(planned_forward_sample_bounds(&plans, true).count(), 2);
    assert_eq!(planned_forward_sample_bounds(&plans, false).count(), 1);
}

// Regression: bind-pose-only bounds omitted rows reached by animated vertices.
#[test]
fn planned_forward_sample_bounds_use_model_pose_envelope() {
    let mut fixed = joints(&[("animated", 1)]);
    fixed.bounds.insert(
        "animated".to_string(),
        Aabb {
            min: Vec3::splat(-6.0),
            max: Vec3::splat(6.0),
        },
    );
    let plans = plan_mesh_frame_plans(&[instance("animated", 10.0, 1)], &[], &fixed);

    let bounds: Vec<_> = planned_forward_sample_bounds(&plans, true).collect();

    assert_eq!(bounds.len(), 1);
    assert_eq!(bounds[0].min, Vec3::new(4.0, -6.0, -6.0));
    assert_eq!(bounds[0].max, Vec3::new(16.0, 6.0, 6.0));
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
    i.dynamic_shadow_visible = false;
    i
}

fn dynamic_shadow_instance(model: &str, x: f32, seed: u32) -> MeshInstanceInput {
    let mut i = non_forward_instance(model, x, seed);
    i.dynamic_shadow_visible = true;
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
fn plan_budgets_explicit_dynamic_caster_before_static_relevance() {
    // Regression: a broad promoted-static retention set could consume the
    // palette before an off-PVS descriptor shadow-only body reached dynamic
    // per-light cone culling.
    let per = (MAX_PALETTE_ENTRIES / 2) as u32;
    let joints = joints(&[("grunt", per)]);
    let instances = [
        non_forward_instance("grunt", 99.0, 2),
        dynamic_shadow_instance("grunt", 1.0, 0),
        dynamic_shadow_instance("grunt", 2.0, 1),
    ];

    let plan = plan_mesh_frame(&instances, &joints);
    let survivors: Vec<_> = plan
        .groups
        .iter()
        .flat_map(|group| &group.instances)
        .collect();

    assert_eq!(survivors.len(), 2);
    assert!(
        survivors
            .iter()
            .all(|instance| instance.dynamic_shadow_visible)
    );
    assert!(survivors.iter().all(|instance| !instance.forward_visible));
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
    viewmodel.presentation = MeshPresentation::Viewmodel;
    let instances = [
        instance("world", 1.0, 0),
        instance("world", 2.0, 1),
        viewmodel,
    ];

    let plans = plan_mesh_frame_plans(&instances, &[], &joints);

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

// Regression: planning every world priority before the viewmodel let a
// shadow-only world group evict the forward-visible first-person weapon.
#[test]
fn plans_apply_visibility_priority_globally_across_world_and_viewmodel() {
    let per = (MAX_PALETTE_ENTRIES / 3) as u32;
    let joints = joints(&[
        ("static-only", per),
        ("dynamic", per),
        ("world-forward", per),
        ("viewmodel", per),
    ]);
    let mut viewmodel = instance("viewmodel", 4.0, 4);
    viewmodel.presentation = MeshPresentation::Viewmodel;
    viewmodel.dynamic_shadow_visible = false;
    let inputs = [
        non_forward_instance("static-only", 1.0, 1),
        dynamic_shadow_instance("dynamic", 2.0, 2),
        instance("world-forward", 3.0, 3),
        viewmodel,
    ];

    let plans = plan_mesh_frame_plans(&inputs, &[], &joints);
    let world_models: Vec<_> = plans
        .world
        .groups
        .iter()
        .map(|group| group.model.as_str())
        .collect();

    assert_eq!(world_models, ["world-forward", "dynamic"]);
    assert_eq!(plans.world.instance_count, 2);
    assert_eq!(plans.world.dropped, 1, "promoted-static-only drops last");
    assert_eq!(plans.viewmodel.instance_count, 1);
    assert_eq!(plans.viewmodel.dropped, 0, "forward viewmodel survives");
    assert_eq!(plans.world.groups[0].instances[0].palette_base, 0);
    assert_eq!(plans.viewmodel.groups[0].instances[0].palette_base, per);
    assert_eq!(plans.world.groups[1].instances[0].palette_base, per * 2);
    assert_eq!(plans.world.groups[0].instance_offset, 0);
    assert_eq!(plans.world.groups[1].instance_offset, 1);
    assert_eq!(plans.viewmodel.groups[0].instance_offset, 2);
}

#[test]
fn plans_separate_adjacent_same_holder_world_and_viewmodel_inputs() {
    // Regression: connected clients can seed the local body and viewmodel
    // with the same raw id. Holder-only grouping merged their adjacent
    // inputs, tripping the plan-consistency assertion in debug builds and
    // routing both through one plan in release builds.
    let holder = 42;
    let joints = joints(&[
        ("world-body", 3),
        ("viewmodel-weapon", 1),
        ("world-other", 2),
    ]);
    let world = instance("world-body", 1.0, holder);
    let mut viewmodel = attachment_instance("viewmodel-weapon", 2.0, holder, usize::MAX);
    viewmodel.presentation = MeshPresentation::Viewmodel;
    viewmodel.dynamic_shadow_visible = false;
    // Force the mixed-order fallback so this test exercises plan-aware
    // atomic grouping rather than only the collector's suffix fast path.
    let world_other = instance("world-other", 3.0, 77);

    let plans = plan_mesh_frame_plans(&[world, viewmodel, world_other], &[], &joints);

    assert_eq!(plans.world.instance_count, 2);
    assert_eq!(plans.viewmodel.instance_count, 1);
    assert_eq!(plans.world.groups[0].model.as_str(), "world-body");
    assert_eq!(plans.viewmodel.groups[0].model.as_str(), "viewmodel-weapon");
    assert!(plans.world.groups[0].instances[0].forward_visible);
    assert!(plans.viewmodel.groups[0].instances[0].forward_visible);
    assert_eq!(plans.world.groups[0].instance_offset, 0);
    assert_eq!(plans.viewmodel.groups[0].instance_offset, 2);
    assert_eq!(plans.world.groups[0].instances[0].palette_base, 0);
    assert_eq!(plans.viewmodel.groups[0].instances[0].palette_base, 5);
    assert!(
        plans
            .world
            .groups
            .iter()
            .all(|group| group.model.as_str() != "viewmodel-weapon"),
        "the world plan is the shadow-depth plan, so it must contain no viewmodel entry",
    );
}

#[test]
fn plans_without_viewmodels_keep_world_priority_and_empty_viewmodel_plan() {
    let joints = joints(&[("static-only", 1), ("dynamic", 1), ("forward", 1)]);
    let inputs = [
        non_forward_instance("static-only", 1.0, 1),
        dynamic_shadow_instance("dynamic", 2.0, 2),
        instance("forward", 3.0, 3),
    ];

    let plans = plan_mesh_frame_plans(&inputs, &[], &joints);
    let planned_models: Vec<_> = plans
        .world
        .groups
        .iter()
        .map(|group| group.model.as_str())
        .collect();

    assert_eq!(planned_models, ["forward", "dynamic", "static-only"]);
    assert_eq!(plans.world.instance_count, 3);
    assert!(plans.viewmodel.groups.is_empty());
    assert_eq!(plans.viewmodel.instance_count, 0);
    assert_eq!(plans.viewmodel.dropped, 0);
}

#[test]
fn missing_viewmodel_asset_leaves_no_viewmodel_plan_to_draw() {
    let joints = joints(&[("world", 2)]);
    let mut missing_viewmodel = instance("missing-viewmodel", 0.0, 2);
    missing_viewmodel.presentation = MeshPresentation::Viewmodel;

    let plans = plan_mesh_frame_plans(
        &[instance("world", 1.0, 1), missing_viewmodel],
        &[],
        &joints,
    );

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
        dynamic_shadow_visible: true,
        mirror: MirrorReach::Hidden,
        ghost_look: None,
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
        dynamic_shadow_visible: true,
        mirror: MirrorReach::Hidden,
        ghost_look: None,
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
        dynamic_shadow_visible: true,
        mirror: MirrorReach::Hidden,
        ghost_look: None,
    };
    assert!(
        instance_casts_into_cone(&inst, &planes),
        "rotated bar on the cone axis must enclose correctly and cast"
    );
}

#[test]
fn plan_stamps_model_local_bounds_onto_planned_instances() {
    // Each planned instance must carry its model's conservative LOCAL-space
    // pose envelope. The planner stamps it from the model-info lookup, so two
    // distinct models' instances carry distinct bounds.
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

fn ghost_look(opacity: f32) -> GhostInstanceLook {
    GhostInstanceLook {
        opacity,
        saturation: 0.0,
        tint: [1.0; 3],
    }
}

/// `input` seen through `link`: the collector's ghost shape, forward
/// visible and never a shadow caster.
fn ghost_of(input: MeshInstanceInput, link: u16, opacity: f32) -> MeshInstanceInput {
    MeshInstanceInput {
        palette_cache_key: input.palette_cache_key.ghost_through(link),
        forward_visible: true,
        dynamic_shadow_visible: false,
        mirror: MirrorReach::Hidden,
        presentation: MeshPresentation::Ghost(ghost_look(opacity)),
        ..input
    }
}

#[test]
fn plans_route_ghosts_to_their_own_plan_never_world_or_shadow() {
    let joints = joints(&[("pawn", 4), ("gun", 1), ("viewmodel", 2)]);
    let mut viewmodel = instance("viewmodel", 0.0, 3);
    viewmodel.presentation = MeshPresentation::Viewmodel;
    viewmodel.dynamic_shadow_visible = false;
    let instances = [
        instance("pawn", 1.0, 7),
        attachment_instance("gun", 1.0, 7, 0),
        viewmodel,
    ];
    // The same pawn and its weapon seen through link 0 and link 1.
    let ghosts = [
        ghost_of(instance("pawn", 101.0, 7), 0, 0.5),
        ghost_of(attachment_instance("gun", 101.0, 7, 0), 0, 0.5),
        ghost_of(instance("pawn", 201.0, 7), 1, 0.25),
        ghost_of(attachment_instance("gun", 201.0, 7, 0), 1, 0.25),
    ];

    let plans = plan_mesh_frame_plans(&instances, &ghosts, &joints);

    // The world plan is the shadow-depth and SH-sampling plan: no ghost key
    // and no ghost look may reach it.
    assert_eq!(plans.world.instance_count, 2);
    for planned in plans.world.groups.iter().flat_map(|group| &group.instances) {
        assert!(planned.ghost_look.is_none());
        assert!(!matches!(
            planned.palette_cache_key,
            MeshPaletteCacheKey::Ghost { .. }
        ));
    }
    assert_eq!(
        planned_forward_sample_bounds(&plans, true).count(),
        3,
        "SH sample bounds cover world and viewmodel instances, never ghosts"
    );
    assert_eq!(plans.viewmodel.instance_count, 1);
    assert_eq!(plans.ghost.instance_count, 4);
    assert_eq!(plans.ghost.dropped, 0);

    let ghost_models: Vec<_> = plans
        .ghost
        .groups
        .iter()
        .map(|group| (group.model.as_str(), group.instances.len()))
        .collect();
    assert_eq!(ghost_models, [("pawn", 2), ("gun", 2)]);
    // Ghost instances follow the world and viewmodel instances in the
    // shared instance buffer, and their palette runs follow theirs.
    assert_eq!(plans.ghost.groups[0].instance_offset, 3);
    assert_eq!(plans.ghost.groups[1].instance_offset, 5);
    let ghost_bases: Vec<u32> = plans
        .ghost
        .groups
        .iter()
        .flat_map(|group| group.instances.iter().map(|planned| planned.palette_base))
        .collect();
    assert_eq!(ghost_bases, [7, 12, 11, 16], "pawn 0, pawn 1, gun 0, gun 1");
    let pawn_ghosts = &plans.ghost.groups[0].instances;
    assert_eq!(pawn_ghosts[0].ghost_look, Some(ghost_look(0.5)));
    assert_eq!(pawn_ghosts[1].ghost_look, Some(ghost_look(0.25)));
    assert_eq!(
        pawn_ghosts[1].palette_cache_key,
        MeshPaletteCacheKey::Ghost {
            link: 1,
            holder: 7,
            attachment_index: None,
        }
    );
    assert!(
        pawn_ghosts
            .iter()
            .all(|planned| !planned.dynamic_shadow_visible)
    );
}

#[test]
fn plan_drops_ghost_groups_before_real_forward_meshes_and_counts_them() {
    // The world fills all but three palette slots. The link-0 ghost group
    // (holder + weapon, 3 slots) fits; the link-1 group of the same pawn is
    // its own atomic group and drops whole.
    let joints = joints(&[
        ("big", MAX_PALETTE_ENTRIES as u32 - 3),
        ("pawn", 2),
        ("gun", 1),
    ]);
    let ghosts = [
        ghost_of(instance("pawn", 1.0, 7), 0, 1.0),
        ghost_of(attachment_instance("gun", 1.0, 7, 0), 0, 1.0),
        ghost_of(instance("pawn", 2.0, 7), 1, 1.0),
        ghost_of(attachment_instance("gun", 2.0, 7, 0), 1, 1.0),
    ];

    let plans = plan_mesh_frame_plans(&[instance("big", 0.0, 1)], &ghosts, &joints);

    assert_eq!(
        plans.world.instance_count, 1,
        "the real mesh is admitted first"
    );
    assert_eq!(plans.world.dropped, 0);
    assert_eq!(plans.ghost.instance_count, 2);
    assert_eq!(
        plans.ghost.dropped, 2,
        "the overflowing ghost group drops whole"
    );
    assert!(
        plans
            .ghost
            .groups
            .iter()
            .flat_map(|group| &group.instances)
            .all(|planned| matches!(
                planned.palette_cache_key,
                MeshPaletteCacheKey::Ghost { link: 0, .. }
            ))
    );
}

// Regression: ghosts planned inside each visibility priority took the budget
// ahead of real dynamic-shadow-only and promoted-static-only meshes.
#[test]
fn plan_drops_ghosts_before_real_shadow_only_meshes() {
    // The three real groups fill the palette but for less than one more
    // group of the same size, so the ghost group cannot fit after them.
    let per = (MAX_PALETTE_ENTRIES / 3) as u32;
    let joints = joints(&[
        ("world-forward", per),
        ("dynamic", per),
        ("static-only", per),
        ("pawn", per),
    ]);
    let inputs = [
        non_forward_instance("static-only", 1.0, 1),
        dynamic_shadow_instance("dynamic", 2.0, 2),
        instance("world-forward", 3.0, 3),
    ];
    let ghosts = [ghost_of(instance("pawn", 4.0, 7), 0, 1.0)];

    let plans = plan_mesh_frame_plans(&inputs, &ghosts, &joints);

    let world_models: Vec<_> = plans
        .world
        .groups
        .iter()
        .map(|group| group.model.as_str())
        .collect();
    assert_eq!(world_models, ["world-forward", "dynamic", "static-only"]);
    assert_eq!(plans.world.dropped, 0, "every real shadow-only mesh plans");
    assert_eq!(plans.ghost.instance_count, 0);
    assert_eq!(plans.ghost.dropped, 1, "the ghost drops instead");
}

/// A world instance only the planar mirror reaches.
fn mirror_only_instance(model: &str, x: f32, seed: u32) -> MeshInstanceInput {
    let mut i = non_forward_instance(model, x, seed);
    i.mirror = MirrorReach::Only;
    i
}

/// Every planned instance across the three plans, by phase seed, with its
/// palette base: the frame's admissions in budget order.
fn admissions(plans: &MeshFramePlans) -> Vec<(u32, u32)> {
    let mut out: Vec<_> = [&plans.world, &plans.viewmodel, &plans.ghost]
        .into_iter()
        .flat_map(|plan| &plan.groups)
        .flat_map(|group| &group.instances)
        .map(|instance| (instance.phase_seed, instance.palette_base))
        .collect();
    out.sort_unstable_by_key(|&(_, base)| base);
    out
}

// Mirror-only instances rank after every main-view admission: forward,
// dynamic shadow-only and promoted-static-only casters, then ghosts. Listed
// first, the mirror-only one still takes the last palette run, and it
// keeps its reach for the mirror pass.
#[test]
fn plan_ranks_mirror_only_instances_after_ghosts_and_shadow_only_casters() {
    let per = (MAX_PALETTE_ENTRIES / 5) as u32;
    let joints = joints(&[
        ("mirror", per),
        ("static-only", per),
        ("dynamic", per),
        ("world-forward", per),
        ("pawn", per),
    ]);
    let inputs = [
        mirror_only_instance("mirror", 0.0, 5),
        non_forward_instance("static-only", 1.0, 1),
        dynamic_shadow_instance("dynamic", 2.0, 2),
        instance("world-forward", 3.0, 3),
    ];
    let ghosts = [ghost_of(instance("pawn", 4.0, 7), 0, 1.0)];

    let plans = plan_mesh_frame_plans(&inputs, &ghosts, &joints);

    let order: Vec<u32> = admissions(&plans).iter().map(|&(seed, _)| seed).collect();
    assert_eq!(
        order,
        [3, 2, 1, 7, 5],
        "forward, dynamic, static, ghost, mirror"
    );
    let mirror = plans
        .world
        .groups
        .iter()
        .flat_map(|group| &group.instances)
        .find(|instance| instance.phase_seed == 5)
        .expect("the mirror-only instance plans with room");
    assert_eq!(mirror.mirror, MirrorReach::Only);
    assert!(!mirror.forward_visible && !mirror.dynamic_shadow_visible);
    assert_eq!(plans.world.dropped + plans.ghost.dropped, 0);

    // The single-plan entry ranks it last too.
    let plan = plan_mesh_frame(&inputs, &joints);
    let last = plan
        .groups
        .iter()
        .flat_map(|group| &group.instances)
        .max_by_key(|instance| instance.palette_base)
        .expect("planned");
    assert_eq!(last.phase_seed, 5);
}

// With the pool full of main-view admissions and ghosts, the mirror-only
// instance is the one dropped: nothing the main view draws or casts, and
// no ghost, yields to it.
#[test]
fn plan_drops_mirror_only_instances_first_when_the_pool_is_full() {
    let per = (MAX_PALETTE_ENTRIES / 4) as u32;
    let joints = joints(&[
        ("mirror", per),
        ("static-only", per),
        ("dynamic", per),
        ("world-forward", per),
        ("pawn", per),
    ]);
    let inputs = [
        mirror_only_instance("mirror", 0.0, 5),
        non_forward_instance("static-only", 1.0, 1),
        dynamic_shadow_instance("dynamic", 2.0, 2),
        instance("world-forward", 3.0, 3),
    ];
    let ghosts = [ghost_of(instance("pawn", 4.0, 7), 0, 1.0)];

    let plans = plan_mesh_frame_plans(&inputs, &ghosts, &joints);

    assert_eq!(plans.world.dropped, 1, "the mirror-only instance drops");
    assert_eq!(plans.ghost.dropped, 0, "the ghost keeps its run");
    let order: Vec<u32> = admissions(&plans).iter().map(|&(seed, _)| seed).collect();
    assert_eq!(order, [3, 2, 1, 7]);
}

// Adding mirror-only instances, with room or past a full pool, changes no
// main-view admission: the same instances plan at the same palette runs,
// in every plan, and an instance the mirror shares with the main view keeps
// its main rank and carries its mirror reach.
#[test]
fn plan_main_view_admissions_unchanged_by_mirror_only_instances() {
    for per in [MAX_PALETTE_ENTRIES / 8, MAX_PALETTE_ENTRIES / 3] {
        let joints = joints(&[("grunt", per as u32), ("gun", 1), ("pawn", per as u32)]);
        let mut shared = instance("grunt", 1.0, 1);
        shared.mirror = MirrorReach::AlsoMain;
        let mut viewmodel = instance("gun", 0.0, 9);
        viewmodel.presentation = MeshPresentation::Viewmodel;
        let main = vec![
            shared,
            dynamic_shadow_instance("grunt", 2.0, 2),
            non_forward_instance("grunt", 3.0, 3),
            instance("grunt", 4.0, 4),
            viewmodel,
        ];
        let ghosts = [ghost_of(instance("pawn", 5.0, 7), 0, 1.0)];
        let mut with_mirror = vec![
            mirror_only_instance("grunt", 10.0, 10),
            mirror_only_instance("grunt", 11.0, 11),
        ];
        with_mirror.extend(main.iter().cloned());

        let before = plan_mesh_frame_plans(&main, &ghosts, &joints);
        let after = plan_mesh_frame_plans(&with_mirror, &ghosts, &joints);

        let after_main: Vec<_> = admissions(&after)
            .into_iter()
            .filter(|&(seed, _)| seed < 10)
            .collect();
        assert_eq!(after_main, admissions(&before), "per {per}");
        assert_eq!(after.viewmodel.dropped, before.viewmodel.dropped);
        assert_eq!(after.ghost.dropped, before.ghost.dropped);
        let shared = after
            .world
            .groups
            .iter()
            .flat_map(|group| &group.instances)
            .find(|instance| instance.phase_seed == 1);
        if let Some(shared) = shared {
            assert_eq!(shared.mirror, MirrorReach::AlsoMain);
            assert!(shared.forward_visible);
        }
        assert!(
            after.viewmodel.groups[0]
                .instances
                .iter()
                .all(|instance| instance.mirror == MirrorReach::Hidden),
            "a viewmodel never carries a mirror reach"
        );
    }
}

// AC 22, the instance cap: a cap full of main-view admissions — forward
// draws, a dynamic shadow-only caster, a promoted-static-only caster and a
// ghost, at one palette run each so the instance cap binds — evicts none
// of them for mirror-only instances, which all drop; the occupancy splits
// the admissions and the drops by rank (AC 39).
#[test]
fn plan_instance_cap_full_of_main_view_admissions_drops_every_mirror_only_instance() {
    let joints = joints(&[("prop", 1), ("pawn", 1), ("mirror", 1)]);
    let mut main: Vec<MeshInstanceInput> = (0..MAX_INSTANCES as u32 - 3)
        .map(|seed| instance("prop", seed as f32, seed))
        .collect();
    main.push(dynamic_shadow_instance("prop", -1.0, 100_000));
    main.push(non_forward_instance("prop", -2.0, 100_001));
    let ghosts = [ghost_of(instance("pawn", -3.0, 100_002), 0, 1.0)];
    let mut with_mirror: Vec<MeshInstanceInput> = (0..3)
        .map(|offset| mirror_only_instance("mirror", 10.0, 200_000 + offset))
        .collect();
    with_mirror.extend(main.iter().cloned());

    let before = plan_mesh_frame_plans(&main, &ghosts, &joints);
    let after = plan_mesh_frame_plans(&with_mirror, &ghosts, &joints);

    assert_eq!(
        before.world.instance_count + before.ghost.instance_count,
        MAX_INSTANCES as u32,
        "the main view fills the cap"
    );
    assert_eq!(admissions(&after), admissions(&before));
    assert_eq!(
        after.occupancy(),
        MeshCapOccupancy {
            main: MAX_INSTANCES as u32,
            mirror_only: 0,
            dropped_main: 0,
            dropped_mirror_only: 3,
        }
    );

    // With room, the mirror-only instances plan and count as the mirror's.
    let room = plan_mesh_frame_plans(&with_mirror[..10], &ghosts, &joints);
    assert_eq!(
        room.occupancy(),
        MeshCapOccupancy {
            main: 8,
            mirror_only: 3,
            dropped_main: 0,
            dropped_mirror_only: 0,
        }
    );
}

// O15: an instance the main view reaches only as a shadow caster, which
// the mirror also draws, keeps its main-view rank: listed after
// mirror-only instances in a pool with room for one more run, it takes
// the run and keeps its mirror reach, and the mirror-only instance drops.
#[test]
fn plan_keeps_main_rank_for_a_shadow_caster_the_mirror_also_draws() {
    let per = (MAX_PALETTE_ENTRIES / 2) as u32;
    let joints = joints(&[("grunt", per), ("mirror", per)]);
    let mut caster = dynamic_shadow_instance("grunt", 1.0, 2);
    caster.mirror = MirrorReach::AlsoMain;
    let inputs = [
        mirror_only_instance("mirror", 0.0, 5),
        instance("grunt", 2.0, 1),
        caster,
    ];

    let plans = plan_mesh_frame_plans(&inputs, &[], &joints);

    let order: Vec<u32> = admissions(&plans).iter().map(|&(seed, _)| seed).collect();
    assert_eq!(order, [1, 2], "forward, then the shared shadow caster");
    let caster = plans
        .world
        .groups
        .iter()
        .flat_map(|group| &group.instances)
        .find(|instance| instance.phase_seed == 2)
        .expect("the caster keeps its main-view run");
    assert_eq!(caster.mirror, MirrorReach::AlsoMain);
    assert!(caster.dynamic_shadow_visible && !caster.forward_visible);
    assert_eq!(plans.dropped_mirror_only, 1);
    assert_eq!(plans.occupancy().dropped_main, 0);
}

#[test]
fn occupancy_counts_admitted_mirror_only_instances_without_scanning_groups() {
    let joints = joints(&[("prop", 1), ("mirror", 1)]);
    let mut inputs = vec![instance("prop", 0.0, 1), instance("prop", 1.0, 2)];
    inputs.push(mirror_only_instance("mirror", 5.0, 3));
    inputs.push(mirror_only_instance("mirror", 6.0, 4));

    let plans = plan_mesh_frame_plans(&inputs, &[], &joints);
    let scanned = plans
        .world
        .groups
        .iter()
        .flat_map(|group| &group.instances)
        .filter(|planned| planned.mirror.is_only())
        .count() as u32;

    assert_eq!(plans.planned_mirror_only, scanned);
    assert_eq!(
        plans.occupancy(),
        MeshCapOccupancy {
            main: 2,
            mirror_only: 2,
            dropped_main: 0,
            dropped_mirror_only: 0,
        }
    );

    let no_mirror = plan_mesh_frame_plans(&inputs[..2], &[], &joints);
    assert_eq!(no_mirror.planned_mirror_only, 0);
    assert_eq!(no_mirror.occupancy().mirror_only, 0);
}
