// Ghost draw order: one unit per ghost, farthest first, across skinned and mover ghosts.
// See: context/lib/rendering_pipeline.md §7.9

use super::*;
use postretro_model::ModelHandle;
use postretro_model::sample_params::MeshSampleParams;
use postretro_render_cpu::mesh_instances::ModelDrawGroup;
use postretro_render_data::cone_frustum::Aabb;

fn ghost_key(link: u16, holder: u32, attachment_index: Option<usize>) -> MeshPaletteCacheKey {
    MeshPaletteCacheKey::Ghost {
        link,
        holder,
        attachment_index,
    }
}

/// A ghost instance whose pose envelope centres at `center`.
fn instance(key: MeshPaletteCacheKey, center: Vec3) -> PlannedInstance {
    PlannedInstance {
        transform: Mat4::from_translation(center),
        shadow_bias_scale: 1.0,
        palette_base: 0,
        phase_seed: 0,
        palette_cache_key: key,
        bounds: Aabb::default(),
        sample: MeshSampleParams::rigid(),
        pose_inputs: None,
        capture: None,
        resample: true,
        forward_visible: true,
        dynamic_shadow_visible: false,
        ghost_look: None,
    }
}

/// A ghost plan of `groups` in order, densely packed from `first_offset`.
fn plan(first_offset: u32, groups: Vec<(&str, Vec<PlannedInstance>)>) -> MeshFramePlan {
    let mut offset = first_offset;
    let groups: Vec<ModelDrawGroup> = groups
        .into_iter()
        .map(|(model, instances)| {
            let group = ModelDrawGroup {
                model: ModelHandle::from(model),
                instance_offset: offset,
                instances,
            };
            offset += group.instances.len() as u32;
            group
        })
        .collect();
    MeshFramePlan {
        instance_count: offset - first_offset,
        groups,
        dropped: 0,
    }
}

fn at_depth(z: f32) -> Vec3 {
    Vec3::new(0.0, 0.0, z)
}

fn span(group: u32, start: u32, end: u32) -> SkinnedGhostSpan {
    SkinnedGhostSpan { group, start, end }
}

// Under the identity view-projection clip depth is z: the farthest ghost
// draws first, whichever kind it is.
#[test]
fn ghost_order_draws_skinned_and_mover_ghosts_farthest_first() {
    let skinned = plan(
        5,
        vec![(
            "enemy",
            vec![instance(ghost_key(0, 7, None), at_depth(0.4))],
        )],
    );
    let mut order = GhostDrawOrder::default();
    order.build(
        Some(&skinned),
        [at_depth(0.2), at_depth(0.8)],
        Mat4::IDENTITY,
    );
    let units: Vec<GhostUnit> = order.units().collect();
    assert_eq!(
        units,
        [
            GhostUnit::Mover { draw: 1 },
            GhostUnit::Skinned { first: 0, count: 1 },
            GhostUnit::Mover { draw: 0 },
        ]
    );
    assert_eq!(order.spans(units[1]), [span(0, 5, 6)]);
    assert!(order.spans(units[0]).is_empty());
}

// A holder and its attachments through one link are one ghost, so a held
// weapon runs its depth and colour draws with its holder. The same holder
// through another link is a second ghost. The holder's centre sorts the
// ghost, not its attachment's, even when the attachment's group plans first.
#[test]
fn ghost_order_groups_a_holder_and_its_attachments_per_link() {
    let skinned = plan(
        10,
        vec![
            (
                "rifle",
                vec![
                    instance(ghost_key(0, 7, Some(0)), at_depth(0.1)),
                    instance(ghost_key(1, 7, Some(0)), at_depth(0.9)),
                ],
            ),
            (
                "enemy",
                vec![
                    instance(ghost_key(0, 7, None), at_depth(0.5)),
                    instance(ghost_key(1, 7, None), at_depth(0.3)),
                ],
            ),
        ],
    );
    let mut order = GhostDrawOrder::default();
    order.build(Some(&skinned), [], Mat4::IDENTITY);
    let units: Vec<GhostUnit> = order.units().collect();
    assert_eq!(units.len(), 2, "two ghosts: one per link");
    // Link 0's holder sits at 0.5, link 1's at 0.3: link 0 draws first.
    assert_eq!(order.spans(units[0]), [span(0, 10, 11), span(1, 12, 13)]);
    assert_eq!(order.spans(units[1]), [span(0, 11, 12), span(1, 13, 14)]);
}

// Consecutive instances of one ghost in one group merge into one instanced
// draw.
#[test]
fn ghost_order_merges_a_ghosts_consecutive_instances_of_one_model() {
    let skinned = plan(
        0,
        vec![(
            "pistol",
            vec![
                instance(ghost_key(2, 4, Some(0)), at_depth(0.5)),
                instance(ghost_key(2, 4, Some(1)), at_depth(0.5)),
                instance(ghost_key(2, 4, None), at_depth(0.5)),
            ],
        )],
    );
    let mut order = GhostDrawOrder::default();
    order.build(Some(&skinned), [], Mat4::IDENTITY);
    let units: Vec<GhostUnit> = order.units().collect();
    assert_eq!(units, [GhostUnit::Skinned { first: 0, count: 1 }]);
    assert_eq!(order.spans(units[0]), [span(0, 0, 3)]);
}

// A ghost whose centre is at or behind the eye wraps the camera: it sorts
// nearest and draws last.
#[test]
fn ghost_order_draws_a_ghost_centred_behind_the_eye_last() {
    let view_proj = glam::camera::rh::proj::directx::perspective(1.2, 1.0, 0.1, 100.0);
    let mut order = GhostDrawOrder::default();
    // Right-handed view: forward is -z, so +z lies behind the eye.
    order.build(
        None,
        [at_depth(2.0), at_depth(-10.0), at_depth(-2.0)],
        view_proj,
    );
    let units: Vec<GhostUnit> = order.units().collect();
    assert_eq!(
        units,
        [
            GhostUnit::Mover { draw: 1 },
            GhostUnit::Mover { draw: 2 },
            GhostUnit::Mover { draw: 0 },
        ]
    );
}

// Equal depths keep build order, so the frame is deterministic.
#[test]
fn ghost_order_keeps_build_order_between_ghosts_at_one_depth() {
    let mut order = GhostDrawOrder::default();
    order.build(None, [at_depth(0.5); 3], Mat4::IDENTITY);
    let units: Vec<GhostUnit> = order.units().collect();
    assert_eq!(
        units,
        [
            GhostUnit::Mover { draw: 0 },
            GhostUnit::Mover { draw: 1 },
            GhostUnit::Mover { draw: 2 },
        ]
    );
}

// A rebuild clears the previous frame and reuses its scratch: a warm frame
// of the same ghosts allocates nothing.
#[test]
fn ghost_order_rebuild_replaces_the_frame_without_growing_its_scratch() {
    let skinned = plan(
        0,
        vec![
            (
                "enemy",
                vec![instance(ghost_key(0, 1, None), at_depth(0.3))],
            ),
            (
                "rifle",
                vec![instance(ghost_key(0, 1, Some(0)), at_depth(0.3))],
            ),
        ],
    );
    let mut order = GhostDrawOrder::default();
    order.build(Some(&skinned), [at_depth(0.6)], Mat4::IDENTITY);
    let capacities = (
        order.units.capacity(),
        order.spans.capacity(),
        order.members.capacity(),
    );
    order.build(Some(&skinned), [at_depth(0.6)], Mat4::IDENTITY);
    assert_eq!(order.units().count(), 2);
    assert_eq!(
        (
            order.units.capacity(),
            order.spans.capacity(),
            order.members.capacity(),
        ),
        capacities
    );
    order.build(None, [], Mat4::IDENTITY);
    assert_eq!(order.units().count(), 0);
}
