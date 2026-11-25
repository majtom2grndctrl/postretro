use glam::Vec3;

use postretro_entities::components::animation::MeshAnimation;
use postretro_entities::components::brain::{ClipClock, Interruption};
use postretro_entities::components::mesh::MeshComponent;

use super::*;
use crate::collision::CollisionWorld;

/// A vertical wall in the plane `x`, spanning `min_y..max_y` and z ±1.
fn wall_world(x: f32, min_y: f32, max_y: f32) -> CollisionWorld {
    let points = vec![
        Vec3::new(x, min_y, -1.0),
        Vec3::new(x, max_y, -1.0),
        Vec3::new(x, max_y, 1.0),
        Vec3::new(x, min_y, 1.0),
    ];
    CollisionWorld::from_triangles_for_test(points, vec![[0u32, 1, 2], [0, 2, 3]])
}

const RADIUS: f32 = 0.15;

fn blocked(world: &CollisionWorld, eye: Vec3, muzzle: Vec3, target: Vec3) -> bool {
    near_path_blocked(
        &CombinedCollisionWorld::static_only(world),
        eye,
        muzzle,
        target,
        RADIUS,
    )
}

// N1: a lip below the muzzle-to-target line but within the shot's radius of
// it holds the shot; raised clear by more than the radius, the line passes.
#[test]
fn a_lip_within_the_shots_radius_of_its_line_holds_it() {
    let eye = Vec3::new(0.0, 1.8, 0.0);
    let muzzle = Vec3::new(0.5, 1.2, 0.0);
    let target = Vec3::new(10.0, 1.2, 0.0);
    // The line runs level at 1.2 m; the lip tops out 0.1 m below it.
    let lip = wall_world(1.0, -1.0, 1.1);
    assert!(
        blocked(&lip, eye, muzzle, target),
        "the lip is inside the shot's radius"
    );
    // A segment test alone would pass it.
    assert!(line_of_sight_combined(
        muzzle,
        target,
        &CombinedCollisionWorld::static_only(&lip)
    ));

    let raised = Vec3::new(0.5, 1.1 + RADIUS + 0.05, 0.0);
    let raised_target = Vec3::new(10.0, raised.y, 0.0);
    assert!(
        !blocked(&lip, eye, raised, raised_target),
        "a line clear of the lip by more than the radius"
    );
}

// N2: a barrel tip pushed through a wall the eye's path does not cross holds
// the shot; moved back, the tip clears it.
#[test]
fn a_barrel_tip_through_a_wall_holds_the_shot_until_it_clears() {
    // The wall rises to 1.5 m at x = 0.4; the eye at 1.8 m sees over it.
    let wall = wall_world(0.4, -1.0, 1.5);
    let eye = Vec3::new(0.0, 1.8, 0.0);
    let target = Vec3::new(10.0, 1.8, 0.0);
    assert!(line_of_sight_combined(
        eye,
        target,
        &CombinedCollisionWorld::static_only(&wall)
    ));

    let through = Vec3::new(0.6, 1.2, 0.0);
    assert!(
        blocked(&wall, eye, through, target),
        "the body-to-muzzle leg crosses the wall"
    );

    // Backed off so the gun sits behind the wall's face… and over it.
    let clear_eye = Vec3::new(0.0, 2.4, 0.0);
    let clear_muzzle = Vec3::new(0.3, 2.0, 0.0);
    assert!(!blocked(
        &wall,
        clear_eye,
        clear_muzzle,
        Vec3::new(10.0, 2.0, 0.0)
    ));
}

// N3: an obstacle just inside the near distance holds the shot; just past it,
// the shot flies; with the aim point nearer, the whole path is tested.
#[test]
fn only_cover_inside_the_near_distance_holds_the_shot() {
    let eye = Vec3::new(0.0, 1.8, 0.0);
    let muzzle = Vec3::new(0.5, 1.2, 0.0);
    let target = Vec3::new(10.0, 1.2, 0.0);
    let inside = wall_world(muzzle.x + NEAR_PATH_DISTANCE_M - 0.3, -1.0, 3.0);
    let past = wall_world(muzzle.x + NEAR_PATH_DISTANCE_M + RADIUS + 0.3, -1.0, 3.0);
    assert!(blocked(&inside, eye, muzzle, target));
    assert!(
        !blocked(&past, eye, muzzle, target),
        "cover past the near distance takes the shot"
    );

    // The aim point nearer than the near distance: the obstacle before it holds.
    let near_target = Vec3::new(muzzle.x + 1.5, 1.2, 0.0);
    let before_near_target = wall_world(muzzle.x + 1.2, -1.0, 3.0);
    assert!(blocked(&before_near_target, eye, muzzle, near_target));
}

fn animated(current: &str) -> MeshComponent {
    MeshComponent::animated(
        "body".to_string(),
        MeshAnimation::new(Default::default(), current.to_string()),
    )
}

fn tracking(state: &str, time: f64) -> ClipClock {
    ClipClock {
        state: Some(state.into()),
        time,
        ..ClipClock::default()
    }
}

#[test]
fn an_uninterrupted_enemy_aims_on_its_tracked_state() {
    let mesh = animated("shoot");
    let clock = tracking("shoot", 0.4);
    assert_eq!(
        classify(
            Some(&mesh),
            &clock,
            clock.interruption,
            false,
            OwnDraw::None,
            0.0
        ),
        Aim::Aiming { time: 0.4 }
    );
}

#[test]
fn a_mesh_without_animation_or_tracked_state_has_no_pose() {
    let clock = tracking("shoot", 0.4);
    assert_eq!(
        classify(None, &clock, clock.interruption, false, OwnDraw::None, 0.0),
        Aim::NoPose
    );
    let stateless = MeshComponent::stateless("body".to_string());
    assert_eq!(
        classify(
            Some(&stateless),
            &clock,
            clock.interruption,
            false,
            OwnDraw::None,
            0.0
        ),
        Aim::NoPose
    );
    assert_eq!(
        classify(
            Some(&animated("idle")),
            &ClipClock::default(),
            Interruption::None,
            false,
            OwnDraw::None,
            0.0
        ),
        Aim::NoPose,
        "an activity that draws no animation tracks no state"
    );
}

#[test]
fn a_foreign_switch_still_drawn_is_not_aiming_until_an_own_draw_lands() {
    let mesh = animated("hit_reaction");
    let mut clock = tracking("shoot", 0.4);
    clock.interruption = Interruption::Foreign;
    assert_eq!(
        classify(
            Some(&mesh),
            &clock,
            clock.interruption,
            false,
            OwnDraw::None,
            0.1
        ),
        Aim::NotAiming
    );
    // The own draw replaces the reaction onto the tracked state: posed where
    // that draw starts the clip (pin NM-26), not where the clock kept it.
    assert_eq!(
        classify(
            Some(&mesh),
            &clock,
            clock.interruption,
            false,
            OwnDraw::Aiming,
            0.1
        ),
        Aim::Aiming { time: 0.1 }
    );
}

#[test]
fn an_airborne_clip_or_pending_redraw_is_not_aiming() {
    let mesh = animated("shoot");
    let clock = tracking("shoot", 0.4);
    assert_eq!(
        classify(
            Some(&mesh),
            &clock,
            clock.interruption,
            true,
            OwnDraw::Aiming,
            0.0
        ),
        Aim::NotAiming,
        "the airborne clip drawn at the launch"
    );
    let mut redraw = tracking("shoot", 0.4);
    redraw.redraw = true;
    assert_eq!(
        classify(
            Some(&mesh),
            &redraw,
            redraw.interruption,
            false,
            OwnDraw::None,
            0.0
        ),
        Aim::NotAiming,
        "a pending redraw, even with the mesh on the tracked state's name (pin NM-25)"
    );
}

#[test]
fn a_failed_own_switch_has_no_pose() {
    let mesh = animated("idle");
    let mut clock = tracking("shoot", 0.4);
    clock.interruption = Interruption::FailedOwnSwitch;
    assert_eq!(
        classify(
            Some(&mesh),
            &clock,
            clock.interruption,
            false,
            OwnDraw::None,
            0.0
        ),
        Aim::NoPose
    );
}

#[test]
fn a_not_aiming_shooter_holds_nothing_and_launches_nothing() {
    let shooter = NpcShooter {
        aim: Aim::NotAiming,
        eye_aim: WeaponAim {
            origin: Vec3::Y,
            direction: Vec3::X,
        },
        eye: Vec3::Y,
        target_aim: Vec3::new(5.0, 1.0, 0.0),
        world: None,
        muzzles: vec![("rifle".into(), Some(Vec3::new(0.3, 0.8, 0.0)))],
    };
    assert_eq!(shooter.decide("rifle", RADIUS), ShotLaunch::NotAiming);
}

#[test]
fn a_missing_muzzle_or_no_pose_launches_from_the_eye_without_a_near_path_test() {
    // A wall right at the gun: a muzzle would be held, the eye shot is not.
    let wall = wall_world(0.4, -1.0, 1.5);
    let world = CombinedCollisionWorld::static_only(&wall);
    let eye_aim = WeaponAim {
        origin: Vec3::new(0.0, 1.8, 0.0),
        direction: Vec3::X,
    };
    let shooter = |aim, muzzle| NpcShooter {
        aim,
        eye_aim,
        eye: eye_aim.origin,
        target_aim: Vec3::new(10.0, 1.8, 0.0),
        world: Some(&world),
        muzzles: vec![("rifle".into(), muzzle)],
    };
    let at_gun = Some(Vec3::new(0.6, 1.2, 0.0));
    assert_eq!(
        shooter(Aim::Aiming { time: 0.0 }, at_gun).decide("rifle", RADIUS),
        ShotLaunch::Held
    );
    assert_eq!(
        shooter(Aim::Aiming { time: 0.0 }, None).decide("rifle", RADIUS),
        ShotLaunch::Launch(eye_aim),
        "a missing muzzle"
    );
    assert_eq!(
        shooter(Aim::NoPose, at_gun).decide("rifle", RADIUS),
        ShotLaunch::Launch(eye_aim),
        "no pose"
    );
    assert_eq!(
        shooter(Aim::Aiming { time: 0.0 }, at_gun).decide("other", RADIUS),
        ShotLaunch::Launch(eye_aim),
        "a weapon with no posed muzzle"
    );
}

#[test]
fn a_clear_muzzle_launches_toward_the_target_aim_point() {
    let shooter = NpcShooter {
        aim: Aim::Aiming { time: 0.0 },
        eye_aim: WeaponAim {
            origin: Vec3::new(0.0, 1.8, 0.0),
            direction: Vec3::X,
        },
        eye: Vec3::new(0.0, 1.8, 0.0),
        target_aim: Vec3::new(10.0, 1.2, 0.0),
        world: None,
        muzzles: vec![("rifle".into(), Some(Vec3::new(0.5, 1.2, 0.0)))],
    };
    assert_eq!(
        shooter.decide("rifle", RADIUS),
        ShotLaunch::Launch(WeaponAim {
            origin: Vec3::new(0.5, 1.2, 0.0),
            direction: Vec3::X,
        })
    );
}

// Regression: at point blank the muzzle-to-target leg swept its full radius
// onto the aim point, so a lintel just above a target's eye, which the shot
// would only graze there, held the shot.
#[test]
fn a_surface_beside_a_point_blank_aim_point_does_not_hold_the_shot() {
    let eye = Vec3::new(0.0, 1.8, 0.0);
    let muzzle = Vec3::new(0.5, 1.2, 0.0);
    // The target's eye 1 m from the muzzle, inside the near distance.
    let target = Vec3::new(1.5, 1.2, 0.0);
    // A lintel 0.1 m above the aim point, within the shot's radius of it,
    // reaching back only to just short of it.
    let lintel = CollisionWorld::from_triangles_for_test(
        vec![
            Vec3::new(1.48, 1.3, -1.0),
            Vec3::new(3.0, 1.3, -1.0),
            Vec3::new(3.0, 1.3, 1.0),
            Vec3::new(1.48, 1.3, 1.0),
        ],
        vec![[0u32, 1, 2], [0, 2, 3]],
    );
    assert!(
        !blocked(&lintel, eye, muzzle, target),
        "a surface the shot only grazes at its aim point leaves the gun clear"
    );
    // An obstacle squarely between the muzzle and the near target still holds.
    let between = wall_world(1.0, -1.0, 3.0);
    assert!(blocked(&between, eye, muzzle, target));
}
