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

/// The radius of an animated model's pose envelope, about the dev player's:
/// its bind pose alone reaches 1.79 m from its feet.
const POSE_RADIUS: f32 = 1.8;

/// A skinned ghost instance as the planner stamps an animated model: its
/// transform at the model's feet, its pose envelope the cube of
/// [`POSE_RADIUS`] centred there. Its sort key is its feet's depth.
fn instance(key: MeshPaletteCacheKey, feet: Vec3) -> PlannedInstance {
    PlannedInstance {
        transform: Mat4::from_translation(feet),
        shadow_bias_scale: 1.0,
        palette_base: 0,
        phase_seed: 0,
        palette_cache_key: key,
        bounds: Aabb {
            min: Vec3::splat(-POSE_RADIUS),
            max: Vec3::splat(POSE_RADIUS),
        },
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

/// A plan of one pawn ghost, [`PAWN`], standing at `feet`.
fn one_pawn(feet: Vec3) -> MeshFramePlan {
    plan(
        0,
        vec![("enemy", vec![instance(ghost_key(0, 3, None), feet)])],
    )
}

fn at_depth(z: f32) -> Vec3 {
    Vec3::new(0.0, 0.0, z)
}

/// A mover ghost whose bounds collapse to one point at `point`.
fn mover_at_point(point: Vec3) -> MoverGhostBounds {
    MoverGhostBounds::new(Mat4::from_translation(point), Aabb::default())
}

/// A mover ghost whose bounds collapse to one point at depth `z`.
fn mover_at(z: f32) -> MoverGhostBounds {
    mover_at_point(at_depth(z))
}

/// The draw order as the sort emits it.
fn units_of(order: &GhostDrawOrder) -> Vec<GhostUnit> {
    order.units().collect()
}

/// An eye for the identity view-projection, which looks along +z: in front of
/// every ghost the tests place under it.
const IDENTITY_EYE: Vec3 = Vec3::new(0.0, 0.0, -1.0);

/// A 16:9 perspective with a 1.2 rad vertical field of view, from the origin
/// looking along −z.
fn wide_view() -> Mat4 {
    glam::camera::rh::proj::directx::perspective(1.2, 16.0 / 9.0, 0.1, 1000.0)
}

/// [`wide_view`]'s projection from `eye`, looking at `target`.
fn view_from(eye: Vec3, target: Vec3) -> Mat4 {
    wide_view() * glam::camera::rh::view::look_at_mat4(eye, target, Vec3::Y)
}

/// An eye 3 m above the floor, looking down at the middle of [`platform`].
const ABOVE_EYE: Vec3 = Vec3::new(0.0, 3.0, 0.0);

fn above_view() -> Mat4 {
    view_from(ABOVE_EYE, Vec3::new(0.0, 0.0, -6.0))
}

/// [`platform`]'s geometry about its origin: 4 × 0.2 × 3 m, its top face at
/// local y 0.
const PLATFORM_LOCAL: Aabb = Aabb {
    min: Vec3::new(-2.0, -0.2, -1.5),
    max: Vec3::new(2.0, 0.0, 1.5),
};

/// A ghost platform 6 m ahead of the origin, its top face at y 0.
fn platform() -> MoverGhostBounds {
    MoverGhostBounds::new(
        Mat4::from_translation(Vec3::new(0.0, 0.0, -6.0)),
        PLATFORM_LOCAL,
    )
}

/// Feet on [`platform`]'s far half, a collision skin (2 cm) above its top.
const FAR_HALF_FEET: Vec3 = Vec3::new(0.0, 0.02, -7.0);

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
        IDENTITY_EYE,
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
    order.build(Some(&skinned), [], Mat4::IDENTITY, IDENTITY_EYE);
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
    order.build(Some(&skinned), [], Mat4::IDENTITY, IDENTITY_EYE);
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
        Vec3::ZERO,
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

// A skinned ghost stands at its holder's origin, the model's feet, not at
// its envelope's bottom, which for an animated model lies a model height
// below the feet. Its body point sits halfway up the envelope's reach.
#[test]
fn ghost_order_stands_a_ghost_at_its_origin_and_aims_at_its_mid_body() {
    let rider = instance(ghost_key(0, 3, None), FAR_HALF_FEET);
    assert_eq!(feet_point(&rider), Some(FAR_HALF_FEET));
    assert_eq!(instance_center(&rider), FAR_HALF_FEET);
    let body = body_point(&rider, FAR_HALF_FEET);
    assert!(body.abs_diff_eq(FAR_HALF_FEET + Vec3::Y * POSE_RADIUS * 0.5, 1e-5));

    let twice = PlannedInstance {
        transform: Mat4::from_scale_rotation_translation(
            Vec3::splat(2.0),
            glam::Quat::from_rotation_y(1.0),
            FAR_HALF_FEET,
        ),
        ..rider.clone()
    };
    assert_eq!(feet_point(&twice), Some(FAR_HALF_FEET));
    let body = body_point(&twice, FAR_HALF_FEET);
    assert!(body.abs_diff_eq(FAR_HALF_FEET + Vec3::Y * POSE_RADIUS, 1e-4));

    // An envelope reaching no higher than the feet aims at the feet.
    let sunk = PlannedInstance {
        bounds: Aabb {
            min: Vec3::splat(-1.0),
            max: Vec3::new(1.0, -0.5, 1.0),
        },
        ..rider.clone()
    };
    assert_eq!(body_point(&sunk, FAR_HALF_FEET), FAR_HALF_FEET);

    let lost = PlannedInstance {
        transform: Mat4::from_translation(Vec3::new(f32::NAN, 0.0, 0.0)),
        ..rider.clone()
    };
    assert_eq!(feet_point(&lost), None);
}

// A platform whose centre is nearer than an animated pawn standing on its
// far half, seen from above, still draws first: the pawn's feet are on it,
// so the pawn blends over it instead of cutting a pawn-shaped hole in it.
// The feet are the pawn's origin; its envelope's bottom lies far below the
// platform.
#[test]
fn ghost_order_draws_a_platform_before_an_animated_pawn_standing_on_its_far_half() {
    let view_proj = above_view();
    let rider = instance(ghost_key(0, 3, None), FAR_HALF_FEET);
    let envelope_bottom = FAR_HALF_FEET - Vec3::Y * POSE_RADIUS;
    let platform = platform();
    assert!(platform.contains(FAR_HALF_FEET, MOVER_CONTACT_MARGIN));
    assert!(!platform.meets_segment(
        envelope_bottom,
        envelope_bottom - Vec3::Y * MOVER_RIDER_REACH_ABOVE,
        MOVER_CONTACT_MARGIN
    ));
    assert!(view_depth(view_proj, platform.center()) < view_depth(view_proj, FAR_HALF_FEET));
    let mut order = GhostDrawOrder::default();
    order.build(
        Some(&plan(0, vec![("enemy", vec![rider])])),
        [platform],
        view_proj,
        ABOVE_EYE,
    );
    assert_eq!(units_of(&order), [MOVER, PAWN]);
}

// A ghost jumping on the platform's far half keeps the platform drawing
// first through the jump: its feet stay within a jump's apex above the top
// face. A ghost hovering well above that is not carried, so the centres
// decide.
#[test]
fn ghost_order_keeps_a_platform_before_a_ghost_jumping_on_it() {
    let view_proj = above_view();
    let platform = platform();
    let mut order = GhostDrawOrder::default();
    for rise in [0.0, 0.5, 1.1] {
        let feet = FAR_HALF_FEET + Vec3::Y * rise;
        assert!(view_depth(view_proj, platform.center()) < view_depth(view_proj, feet));
        order.build(Some(&one_pawn(feet)), [platform], view_proj, ABOVE_EYE);
        assert_eq!(units_of(&order), [MOVER, PAWN], "feet {rise} m up");
    }

    let hovering = Vec3::new(0.0, 2.02, -7.4);
    assert!(view_depth(view_proj, platform.center()) < view_depth(view_proj, hovering));
    order.build(Some(&one_pawn(hovering)), [platform], view_proj, ABOVE_EYE);
    assert_eq!(units_of(&order), [PAWN, MOVER]);
}

// A lift floor turned about world up, its rider standing a collision skin
// above it on its far half, the camera above: the oriented contact test
// still finds the feet, so the floor draws first though its centre is
// nearer.
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
    let rider = instance(ghost_key(0, 3, None), feet);
    assert!(view_depth(view_proj, lift.center()) < view_depth(view_proj, instance_center(&rider)));
    let mut order = GhostDrawOrder::default();
    order.build(
        Some(&plan(0, vec![("enemy", vec![rider])])),
        [lift],
        view_proj,
        Vec3::ZERO,
    );
    assert_eq!(units_of(&order), [MOVER, PAWN]);
}

// The camera on the ground under a 6 × 0.3 × 2 m ghost bridge whose top is
// 3 m up, looking up at a ghost standing on it past the bridge's centre.
// The bridge carries the ghost, but the camera sees it through the bridge,
// so the centres decide: the ghost draws first and the bridge blends over
// its legs, instead of hiding them.
#[test]
fn ghost_order_draws_a_pawn_on_a_bridge_before_the_bridge_seen_from_below() {
    let eye = Vec3::new(0.0, 1.6, 2.5);
    let view_proj = view_from(eye, Vec3::new(0.0, 3.6, -0.2));
    let bridge = MoverGhostBounds::new(
        Mat4::from_translation(Vec3::new(0.0, 2.85, 0.0)),
        Aabb {
            min: Vec3::new(-3.0, -0.15, -1.0),
            max: Vec3::new(3.0, 0.15, 1.0),
        },
    );
    let feet = Vec3::new(0.0, 3.02, -0.2);
    assert!(bridge.contains(feet, MOVER_CONTACT_MARGIN));
    assert!(view_depth(view_proj, bridge.center()) < view_depth(view_proj, feet));
    let mut order = GhostDrawOrder::default();
    order.build(Some(&one_pawn(feet)), [bridge], view_proj, eye);
    assert_eq!(units_of(&order), [PAWN, MOVER]);
}

// A pawn standing inside a closed ghost cab on its far side, the camera
// outside: the camera sees the pawn through the cab's near wall, so the
// pawn draws first and the wall blends over it.
#[test]
fn ghost_order_draws_a_pawn_in_a_cab_before_the_cab_seen_from_outside() {
    let view_proj = wide_view();
    let cab = MoverGhostBounds::new(
        Mat4::from_translation(Vec3::new(0.0, 0.0, -8.0)),
        Aabb {
            min: Vec3::splat(-1.5),
            max: Vec3::splat(1.5),
        },
    );
    let feet = Vec3::new(0.5, -1.48, -9.0);
    assert!(cab.contains(feet, MOVER_CONTACT_MARGIN));
    assert!(view_depth(view_proj, cab.center()) < view_depth(view_proj, feet));
    let mut order = GhostDrawOrder::default();
    order.build(Some(&one_pawn(feet)), [cab], view_proj, Vec3::ZERO);
    assert_eq!(units_of(&order), [PAWN, MOVER]);
}

// The camera inside a 4 × 3 × 4 m ghost cab, a pawn standing in it on its
// far side: the camera sees the pawn from inside, so the cab draws first
// though its centre is nearer, and the pawn blends over its far wall.
#[test]
fn ghost_order_draws_a_cab_around_the_camera_before_a_pawn_standing_in_it() {
    let view_proj = wide_view();
    let cab = MoverGhostBounds::new(
        Mat4::from_translation(Vec3::new(0.0, 0.0, -1.0)),
        Aabb {
            min: Vec3::new(-2.0, -1.5, -2.0),
            max: Vec3::new(2.0, 1.5, 2.0),
        },
    );
    let feet = Vec3::new(0.5, -1.48, -2.5);
    assert!(cab.contains(Vec3::ZERO, 0.0));
    assert!(view_depth(view_proj, cab.center()) < view_depth(view_proj, feet));
    let mut order = GhostDrawOrder::default();
    order.build(Some(&one_pawn(feet)), [cab], view_proj, Vec3::ZERO);
    assert_eq!(units_of(&order), [MOVER, PAWN]);
}

// The camera inside a ghost cab, and a ghost lift floor inside the cab
// carrying a ghost that stands in the cab too. Both movers lift to just
// past the rider; the floor, centred farther, still draws before the cab,
// whichever draw slot each holds, so the cab never hides the floor.
#[test]
fn ghost_order_keeps_two_movers_lifted_past_one_rider_in_their_own_order() {
    let view_proj = wide_view();
    let cab = MoverGhostBounds::new(
        Mat4::from_translation(Vec3::new(0.0, 0.0, -1.0)),
        Aabb {
            min: Vec3::new(-2.0, -1.5, -2.0),
            max: Vec3::new(2.0, 1.5, 2.0),
        },
    );
    let floor = MoverGhostBounds::new(
        Mat4::from_translation(Vec3::new(0.0, -1.45, -2.2)),
        Aabb {
            min: Vec3::new(-1.0, -0.05, -0.6),
            max: Vec3::new(1.0, 0.05, 0.6),
        },
    );
    let feet = Vec3::new(0.2, -1.38, -2.4);
    assert!(cab.contains(feet, MOVER_CONTACT_MARGIN));
    assert!(floor.contains(feet, MOVER_CONTACT_MARGIN));
    assert!(view_depth(view_proj, cab.center()) < view_depth(view_proj, floor.center()));
    assert!(view_depth(view_proj, floor.center()) < view_depth(view_proj, feet));
    let riding = one_pawn(feet);
    let mut order = GhostDrawOrder::default();
    for (movers, floor_slot, cab_slot) in [([cab, floor], 1, 0), ([floor, cab], 0, 1)] {
        order.build(Some(&riding), movers, view_proj, Vec3::ZERO);
        assert_eq!(
            units_of(&order),
            [
                GhostUnit::Mover { draw: floor_slot },
                GhostUnit::Mover { draw: cab_slot },
                PAWN,
            ]
        );
    }
}

// A mover reaching from behind the eye to far ahead, below it, its centre
// nearer than a pawn standing on its far part, still draws before that
// pawn. A mover wholly behind the eye draws last.
#[test]
fn ghost_order_draws_a_mover_reaching_past_the_eye_before_a_pawn_on_its_far_part() {
    let view_proj = glam::camera::rh::proj::directx::perspective(1.2, 1.0, 0.1, 100.0);
    let feet = Vec3::new(0.0, -1.0, -12.0);
    let pawn = plan(
        0,
        vec![("enemy", vec![instance(ghost_key(0, 3, None), feet)])],
    );
    // Right-handed view: forward is -z, so +z lies behind the eye.
    let reaching = MoverGhostBounds::new(
        Mat4::IDENTITY,
        Aabb {
            min: Vec3::new(-1.0, -2.0, -20.0),
            max: Vec3::new(1.0, -1.0, 3.0),
        },
    );
    let behind = MoverGhostBounds::new(
        Mat4::IDENTITY,
        Aabb {
            min: Vec3::new(-1.0, -2.0, 2.0),
            max: Vec3::new(1.0, -1.0, 4.0),
        },
    );
    assert!(view_depth(view_proj, reaching.center()) < view_depth(view_proj, feet));
    let mut order = GhostDrawOrder::default();
    order.build(Some(&pawn), [behind, reaching], view_proj, Vec3::ZERO);
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
                instance(ghost_key(0, 1, None), Vec3::new(-1.0, 0.02, -5.4)),
                instance(ghost_key(0, 2, None), Vec3::new(1.0, 0.02, -6.6)),
                instance(ghost_key(0, 3, None), Vec3::new(3.0, 0.02, -7.0)),
            ],
        )],
    );
    let mut order = GhostDrawOrder::default();
    order.build(Some(&riders), [platform()], above_view(), ABOVE_EYE);
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

// A 2 m ghost door 4 m ahead, turned 45°, and a pawn standing half a metre
// behind it along its normal. The door is nearer across the pawn's
// silhouette, so the pawn draws first and the door blends over it. The
// pawn's feet lie inside the door's world-axis enclosure but not the door,
// and the door's farthest corner lies past the pawn.
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
    let feet = transform.transform_point3(Vec3::new(0.0, -1.0, -0.5));
    let waiting = instance(ghost_key(0, 3, None), feet);
    let pawn_depth = view_depth(view_proj, feet);

    assert_eq!(feet_point(&waiting), Some(feet));
    let enclosure = slab.transformed(&transform);
    assert!(enclosure.min.cmple(feet).all() && enclosure.max.cmpge(feet).all());
    assert!(!door.meets_segment(
        feet,
        feet - Vec3::Y * MOVER_RIDER_REACH_ABOVE,
        MOVER_CONTACT_MARGIN
    ));
    assert!(farthest_corner_depth(view_proj, transform, slab) > pawn_depth);
    assert!(view_depth(view_proj, door.center()) < pawn_depth);

    let mut order = GhostDrawOrder::default();
    order.build(
        Some(&plan(0, vec![("enemy", vec![waiting])])),
        [door],
        view_proj,
        Vec3::ZERO,
    );
    assert_eq!(units_of(&order), [PAWN, MOVER]);
}

// The camera inside a 4 × 3 × 4 m ghost cab, looking 45° into a corner, and a
// pawn standing just outside a wall, 23° off the view axis. The cab is
// centred on the eye, so it draws last and its wall blends over the pawn,
// though its farthest corner lies past the pawn.
#[test]
fn ghost_order_draws_a_cab_around_the_camera_after_a_pawn_outside_its_wall() {
    let view_proj = wide_view();
    let transform = Mat4::from_rotation_y(-FRAC_PI_4);
    let local = Aabb {
        min: Vec3::new(-2.0, -1.5, -2.0),
        max: Vec3::new(2.0, 1.5, 2.0),
    };
    let cab = MoverGhostBounds::new(transform, local);
    let feet = transform.transform_point3(Vec3::new(-2.5, -1.5, -1.0));
    let outside = instance(ghost_key(0, 3, None), feet);
    let pawn_depth = view_depth(view_proj, feet);
    let off_axis = feet.x.abs().atan2(-feet.z).to_degrees();
    assert!((off_axis - 23.2).abs() < 0.1, "{off_axis}");
    assert!(farthest_corner_depth(view_proj, transform, local) > pawn_depth);
    assert_eq!(view_depth(view_proj, cab.center()), f32::NEG_INFINITY);

    let mut order = GhostDrawOrder::default();
    order.build(
        Some(&plan(0, vec![("enemy", vec![outside])])),
        [cab],
        view_proj,
        Vec3::ZERO,
    );
    assert_eq!(units_of(&order), [PAWN, MOVER]);
}

// No contact without a sound shape: a mover flattened to a sheet (no
// inverse) under a pawn's feet, a pawn with no finite bounds whose origin
// sits on a platform, and a mover with no finite bounds. Each mover keeps
// its centre key, and nothing panics; the sound platform alone lifts.
#[test]
fn ghost_order_lifts_no_mover_without_an_inverse_or_finite_bounds() {
    let view_proj = above_view();
    let standing = plan(
        0,
        vec![(
            "enemy",
            vec![instance(ghost_key(0, 3, None), FAR_HALF_FEET)],
        )],
    );
    let mut order = GhostDrawOrder::default();
    order.build(Some(&standing), [platform()], view_proj, ABOVE_EYE);
    assert_eq!(units_of(&order), [MOVER, PAWN]);

    let sheet = MoverGhostBounds::new(
        Mat4::from_translation(Vec3::new(0.0, 0.0, -6.0))
            * Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0)),
        PLATFORM_LOCAL,
    );
    order.build(Some(&standing), [sheet], view_proj, ABOVE_EYE);
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
                ..instance(ghost_key(0, 3, None), FAR_HALF_FEET)
            }],
        )],
    );
    assert!(platform().contains(FAR_HALF_FEET, MOVER_CONTACT_MARGIN));
    order.build(Some(&shapeless), [platform()], view_proj, ABOVE_EYE);
    assert_eq!(units_of(&order), [PAWN, MOVER]);

    let nan_mover = MoverGhostBounds::new(
        Mat4::from_translation(Vec3::new(0.0, 0.0, -6.0)),
        Aabb {
            min: Vec3::splat(f32::NAN),
            max: Vec3::ONE,
        },
    );
    order.build(
        Some(&standing),
        [nan_mover, mover_at_point(Vec3::new(0.0, 0.0, -3.0))],
        view_proj,
        ABOVE_EYE,
    );
    assert_eq!(
        units_of(&order),
        [PAWN, GhostUnit::Mover { draw: 1 }, MOVER]
    );
}

// Equal depths keep build order, so the frame is deterministic.
#[test]
fn ghost_order_keeps_build_order_between_ghosts_at_one_depth() {
    let mut order = GhostDrawOrder::default();
    order.build(None, [mover_at(0.5); 3], Mat4::IDENTITY, IDENTITY_EYE);
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
    order.build(
        Some(&skinned),
        [mover_at(0.6)],
        Mat4::IDENTITY,
        IDENTITY_EYE,
    );
    let capacities = (
        order.units.capacity(),
        order.spans.capacity(),
        order.members.capacity(),
        order.footings.capacity(),
    );
    order.build(
        Some(&skinned),
        [mover_at(0.6)],
        Mat4::IDENTITY,
        IDENTITY_EYE,
    );
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
    order.build(None, [], Mat4::IDENTITY, IDENTITY_EYE);
    assert_eq!(order.units().count(), 0);
}
