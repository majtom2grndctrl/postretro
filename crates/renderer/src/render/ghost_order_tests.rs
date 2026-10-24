// Ghost draw order: one unit per ghost, farthest first, across both kinds.
// See: context/lib/rendering_pipeline.md §7.9

use super::*;
use postretro_model::ModelHandle;
use postretro_model::sample_params::MeshSampleParams;
use postretro_render_cpu::mesh_instances::ModelDrawGroup;
use postretro_render_data::cone_frustum::Aabb;
use std::f32::consts::FRAC_PI_4;

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

/// A 1.8 m pawn ghost centred at `center`: its feet sit 0.9 m below it.
fn pawn(key: MeshPaletteCacheKey, center: Vec3) -> PlannedInstance {
    PlannedInstance {
        bounds: Aabb {
            min: Vec3::new(-0.3, -0.9, -0.3),
            max: Vec3::new(0.3, 0.9, 0.3),
        },
        ..instance(key, center)
    }
}

/// A [`pawn`] standing with its feet at `feet`.
fn pawn_standing_at(key: MeshPaletteCacheKey, feet: Vec3) -> PlannedInstance {
    pawn(key, feet + Vec3::Y * 0.9)
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

/// A mover ghost whose bounds collapse to one point at depth `z`.
fn mover_at(z: f32) -> MoverGhostBounds {
    MoverGhostBounds::new(Mat4::from_translation(at_depth(z)), Aabb::default())
}

/// The draw order as the sort emits it.
fn units_of(order: &GhostDrawOrder) -> Vec<GhostUnit> {
    order.units().collect()
}

/// A 16:9 perspective with a 1.2 rad vertical field of view.
fn wide_view() -> Mat4 {
    glam::camera::rh::proj::directx::perspective(1.2, 16.0 / 9.0, 0.1, 1000.0)
}

/// Depth of the farthest of a placed box's corners in front of the eye.
fn farthest_corner_depth(view_proj: Mat4, transform: Mat4, local: Aabb) -> f32 {
    (0..8)
        .map(|corner: u32| {
            let pick = |bit: u32, min: f32, max: f32| if corner & bit == 0 { min } else { max };
            let point = Vec3::new(
                pick(1, local.min.x, local.max.x),
                pick(2, local.min.y, local.max.y),
                pick(4, local.min.z, local.max.z),
            );
            view_depth(view_proj, transform.transform_point3(point))
        })
        .fold(f32::NEG_INFINITY, f32::max)
}

const PAWN: GhostUnit = GhostUnit::Skinned { first: 0, count: 1 };
const MOVER: GhostUnit = GhostUnit::Mover { draw: 0 };

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
        [mover_at(0.2), mover_at(0.8)],
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
        [mover_at(2.0), mover_at(-10.0), mover_at(-2.0)],
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

// A platform whose bounds centre is nearer than the pawn standing on its far
// half still draws first, because the pawn's feet are on it; the pawn then
// blends over it instead of cutting a pawn-shaped hole in it.
#[test]
fn ghost_order_draws_a_platform_before_a_pawn_standing_on_its_far_half() {
    let pawn = plan(
        0,
        vec![(
            "enemy",
            vec![pawn_standing_at(
                ghost_key(0, 3, None),
                Vec3::new(0.0, 0.0, 0.7),
            )],
        )],
    );
    // Local depth −0.4..0.4 moved to 0.5: the top face at y 0 spans 0.1..0.9.
    let transform = Mat4::from_translation(at_depth(0.5));
    let platform = MoverGhostBounds::new(
        transform,
        Aabb {
            min: Vec3::new(-1.0, -0.1, -0.4),
            max: Vec3::new(1.0, 0.0, 0.4),
        },
    );
    assert!(view_depth(Mat4::IDENTITY, platform.center()) < 0.7);
    let mut order = GhostDrawOrder::default();
    order.build(Some(&pawn), [platform], Mat4::IDENTITY);
    assert_eq!(units_of(&order), [MOVER, PAWN]);
}

// A lift floor turned about world up, its rider standing a collision skin
// above it on its far half: the oriented contact test still finds the feet,
// so the floor draws first though its centre is nearer.
#[test]
fn ghost_order_draws_a_turned_lift_before_a_pawn_riding_its_far_half() {
    let view_proj = wide_view();
    let transform = Mat4::from_translation(Vec3::new(0.0, -1.5, -6.0))
        * Mat4::from_rotation_y(30f32.to_radians());
    let lift = MoverGhostBounds::new(
        transform,
        Aabb {
            min: Vec3::new(-1.5, -0.2, -1.5),
            max: Vec3::new(1.5, 0.0, 1.5),
        },
    );
    let feet = transform.transform_point3(Vec3::new(0.3, 0.02, -1.2));
    let rider = pawn_standing_at(ghost_key(0, 3, None), feet);
    assert!(view_depth(view_proj, lift.center()) < view_depth(view_proj, instance_center(&rider)));
    let mut order = GhostDrawOrder::default();
    order.build(
        Some(&plan(0, vec![("enemy", vec![rider])])),
        [lift],
        view_proj,
    );
    assert_eq!(units_of(&order), [MOVER, PAWN]);
}

// A pawn standing inside a ghost cab on its far side, the camera outside:
// the cab draws first though its centre is nearer.
#[test]
fn ghost_order_draws_a_cab_before_a_pawn_standing_inside_it() {
    let view_proj = wide_view();
    let cab = MoverGhostBounds::new(
        Mat4::from_translation(Vec3::new(0.0, 0.0, -8.0)),
        Aabb {
            min: Vec3::splat(-1.5),
            max: Vec3::splat(1.5),
        },
    );
    let inside = pawn_standing_at(ghost_key(0, 3, None), Vec3::new(0.5, -1.5, -9.0));
    assert!(view_depth(view_proj, cab.center()) < view_depth(view_proj, instance_center(&inside)));
    let mut order = GhostDrawOrder::default();
    order.build(
        Some(&plan(0, vec![("enemy", vec![inside])])),
        [cab],
        view_proj,
    );
    assert_eq!(units_of(&order), [MOVER, PAWN]);
}

// A mover reaching from behind the eye to far ahead, its centre nearer than
// a pawn standing on its far part, still draws before that pawn. A mover
// wholly behind the eye draws last.
#[test]
fn ghost_order_draws_a_mover_reaching_past_the_eye_before_a_pawn_on_its_far_part() {
    let view_proj = glam::camera::rh::proj::directx::perspective(1.2, 1.0, 0.1, 100.0);
    let pawn = plan(
        0,
        vec![(
            "enemy",
            vec![pawn_standing_at(ghost_key(0, 3, None), at_depth(-12.0))],
        )],
    );
    // Right-handed view: forward is -z, so +z lies behind the eye.
    let reaching = MoverGhostBounds::new(
        Mat4::IDENTITY,
        Aabb {
            min: Vec3::new(-1.0, -1.0, -20.0),
            max: Vec3::new(1.0, 0.0, 3.0),
        },
    );
    let behind = MoverGhostBounds::new(
        Mat4::IDENTITY,
        Aabb {
            min: Vec3::new(-1.0, -1.0, 2.0),
            max: Vec3::new(1.0, 0.0, 4.0),
        },
    );
    assert!(
        view_depth(view_proj, reaching.center())
            < view_depth(view_proj, instance_center(&pawn.groups[0].instances[0]))
    );
    let mut order = GhostDrawOrder::default();
    order.build(Some(&pawn), [behind, reaching], view_proj);
    assert_eq!(
        units_of(&order),
        [
            GhostUnit::Mover { draw: 1 },
            PAWN,
            GhostUnit::Mover { draw: 0 },
        ]
    );
}

// A mover carrying several ghosts draws just before the farthest of them; a
// ghost beside it, its feet past the mover's edge, neither pulls it nor
// moves.
#[test]
fn ghost_order_draws_a_platform_before_its_farthest_rider_and_after_a_ghost_beside_it() {
    let riders = plan(
        0,
        vec![(
            "enemy",
            vec![
                pawn_standing_at(ghost_key(0, 1, None), Vec3::new(-1.0, 0.0, 0.6)),
                pawn_standing_at(ghost_key(0, 2, None), Vec3::new(1.0, 0.0, 0.8)),
                pawn_standing_at(ghost_key(0, 3, None), Vec3::new(3.0, 0.0, 0.9)),
            ],
        )],
    );
    let platform = MoverGhostBounds::new(
        Mat4::from_translation(at_depth(0.5)),
        Aabb {
            min: Vec3::new(-2.0, -0.1, -0.5),
            max: Vec3::new(2.0, 0.0, 0.5),
        },
    );
    let mut order = GhostDrawOrder::default();
    order.build(Some(&riders), [platform], Mat4::IDENTITY);
    assert_eq!(
        units_of(&order),
        [
            GhostUnit::Skinned { first: 2, count: 1 },
            MOVER,
            GhostUnit::Skinned { first: 1, count: 1 },
            GhostUnit::Skinned { first: 0, count: 1 },
        ]
    );
}

// A 2 m ghost door 4 m ahead, turned 45°, and a pawn half a metre behind it
// along its normal. The door is nearer across the pawn's silhouette, so the
// pawn draws first and the door blends over it. The pawn's feet lie inside
// the door's world-axis enclosure but not the door, and the door's farthest
// corner lies past the pawn.
#[test]
fn ghost_order_draws_a_pawn_close_behind_an_angled_door_before_the_door() {
    let view_proj = wide_view();
    let transform =
        Mat4::from_translation(Vec3::new(0.0, 0.0, -4.0)) * Mat4::from_rotation_y(FRAC_PI_4);
    let slab = Aabb {
        min: Vec3::new(-1.0, -1.0, -0.05),
        max: Vec3::new(1.0, 1.0, 0.05),
    };
    let door = MoverGhostBounds::new(transform, slab);
    let behind = transform.transform_point3(Vec3::new(0.0, 0.0, -0.5));
    let waiting = pawn(ghost_key(0, 3, None), behind);
    let pawn_depth = view_depth(view_proj, behind);

    let feet = feet_point(&waiting).expect("a finite pawn stands somewhere");
    let enclosure = slab.transformed(&transform);
    assert!(enclosure.min.cmple(feet).all() && enclosure.max.cmpge(feet).all());
    assert!(!door.contains(feet, MOVER_CONTACT_MARGIN));
    assert!(farthest_corner_depth(view_proj, transform, slab) > pawn_depth);
    assert!(view_depth(view_proj, door.center()) < pawn_depth);

    let mut order = GhostDrawOrder::default();
    order.build(
        Some(&plan(0, vec![("enemy", vec![waiting])])),
        [door],
        view_proj,
    );
    assert_eq!(units_of(&order), [PAWN, MOVER]);
}

// The camera inside a 4 × 3 × 4 m ghost cab, looking 45° into a corner, and a
// pawn just outside a wall, 23° off the view axis. The cab is centred on the
// eye, so it draws last and its wall blends over the pawn, though its
// farthest corner lies past the pawn.
#[test]
fn ghost_order_draws_a_cab_around_the_camera_after_a_pawn_outside_its_wall() {
    let view_proj = wide_view();
    let transform = Mat4::from_rotation_y(-FRAC_PI_4);
    let local = Aabb {
        min: Vec3::new(-2.0, -1.5, -2.0),
        max: Vec3::new(2.0, 1.5, 2.0),
    };
    let cab = MoverGhostBounds::new(transform, local);
    let outside_center = transform.transform_point3(Vec3::new(-2.5, 0.0, -1.0));
    let outside = pawn(ghost_key(0, 3, None), outside_center);
    let pawn_depth = view_depth(view_proj, outside_center);
    let off_axis = outside_center.x.abs().atan2(-outside_center.z).to_degrees();
    assert!((off_axis - 23.2).abs() < 0.1, "{off_axis}");
    assert!(farthest_corner_depth(view_proj, transform, local) > pawn_depth);
    assert_eq!(view_depth(view_proj, cab.center()), f32::NEG_INFINITY);

    let mut order = GhostDrawOrder::default();
    order.build(
        Some(&plan(0, vec![("enemy", vec![outside])])),
        [cab],
        view_proj,
    );
    assert_eq!(units_of(&order), [PAWN, MOVER]);
}

// No contact without a sound shape: a mover flattened to a sheet (no
// inverse) under a pawn's feet, a pawn with no finite bounds whose origin
// sits on a platform, and a mover with no finite bounds. Each mover keeps
// its centre key, and nothing panics.
#[test]
fn ghost_order_lifts_no_mover_without_an_inverse_or_finite_bounds() {
    let platform_local = Aabb {
        min: Vec3::new(-1.0, -0.1, -0.4),
        max: Vec3::new(1.0, 0.0, 0.4),
    };
    let standing = plan(
        0,
        vec![(
            "enemy",
            vec![pawn_standing_at(
                ghost_key(0, 3, None),
                Vec3::new(0.0, 0.0, 0.7),
            )],
        )],
    );
    let sheet = MoverGhostBounds::new(
        Mat4::from_translation(at_depth(0.5)) * Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0)),
        platform_local,
    );
    let mut order = GhostDrawOrder::default();
    order.build(Some(&standing), [sheet], Mat4::IDENTITY);
    assert_eq!(units_of(&order), [PAWN, MOVER]);

    let shapeless = plan(
        0,
        vec![(
            "enemy",
            vec![PlannedInstance {
                bounds: Aabb {
                    min: Vec3::splat(f32::NAN),
                    max: Vec3::ZERO,
                },
                ..instance(ghost_key(0, 3, None), at_depth(0.7))
            }],
        )],
    );
    let platform = MoverGhostBounds::new(Mat4::from_translation(at_depth(0.5)), platform_local);
    assert!(platform.contains(at_depth(0.7), MOVER_CONTACT_MARGIN));
    order.build(Some(&shapeless), [platform], Mat4::IDENTITY);
    assert_eq!(units_of(&order), [PAWN, MOVER]);

    let nan_mover = MoverGhostBounds::new(
        Mat4::from_translation(at_depth(0.5)),
        Aabb {
            min: Vec3::splat(f32::NAN),
            max: Vec3::ONE,
        },
    );
    order.build(Some(&standing), [nan_mover, mover_at(0.2)], Mat4::IDENTITY);
    assert_eq!(
        units_of(&order),
        [PAWN, GhostUnit::Mover { draw: 1 }, MOVER]
    );
}

// Equal depths keep build order, so the frame is deterministic.
#[test]
fn ghost_order_keeps_build_order_between_ghosts_at_one_depth() {
    let mut order = GhostDrawOrder::default();
    order.build(None, [mover_at(0.5); 3], Mat4::IDENTITY);
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
    order.build(Some(&skinned), [mover_at(0.6)], Mat4::IDENTITY);
    let capacities = (
        order.units.capacity(),
        order.spans.capacity(),
        order.members.capacity(),
        order.footings.capacity(),
    );
    order.build(Some(&skinned), [mover_at(0.6)], Mat4::IDENTITY);
    assert_eq!(order.units().count(), 2);
    assert_eq!(order.footings.len(), 1);
    assert_eq!(
        (
            order.units.capacity(),
            order.spans.capacity(),
            order.members.capacity(),
            order.footings.capacity(),
        ),
        capacities
    );
    order.build(None, [], Mat4::IDENTITY);
    assert_eq!(order.units().count(), 0);
}
