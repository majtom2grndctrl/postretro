// Where an NPC's projectile shot leaves, and whether it may leave at all:
// the interim aiming rule, the fired weapon's posed muzzle, and the near-path
// gate that holds a shot cover close to the gun would catch.
// See: context/lib/entity_model.md §2 (Weapon vocabulary, Weapon mounts) · §7c

use glam::{Mat4, Vec3};

use postretro_entities::components::brain::{ClipClock, Interruption};
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{EntityId, EntityRegistry, EntityTypeDescriptor, Transform};

use crate::collision::moving::CombinedCollisionWorld;
use crate::collision::shot_queries::{cast_sphere_exact_combined, line_of_sight_combined};
use crate::scripting::builtins::npc_mounts::npc_muzzle_mount;
use crate::scripting_systems::hit_zones::{HitZoneStore, model_matrix};
use crate::scripting_systems::muzzle_pose::{ClipPose, posed_muzzle_world};
use crate::weapon_controller::WeaponAim;

/// How far along the shot's path, from the muzzle, cover holds the shot. An
/// NPC's own cover sits within about 1 m of its muzzle; the dev rifles engage
/// at 12–20 m. Engine floor (`scripting.md` §1): an NPC never shoots its own
/// cover, so no knob is exposed.
pub(crate) const NEAR_PATH_DISTANCE_M: f32 = 2.0;

/// Below this a path leg has no length to test.
const DEGENERATE_LEG_M: f32 = 1e-4;

/// What an enemy's pose allows at its launch: the interim classification,
/// read from explicit state only, until the pose and stagger rules land
/// (`entity_model.md` §7c).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Aim {
    /// The shot leaves the muzzle posed on the tracked state at `time`.
    Aiming { time: f64 },
    /// Nothing launches, from the eye or the muzzle.
    NotAiming,
    /// No pose to read: the shot leaves the eye, as a missing muzzle.
    NoPose,
}

/// The brain's own draw that lands at this tick's apply, after the launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnDraw {
    /// No own draw, or one that does not make the tick aiming (a redraw, an
    /// airborne clip starting or ending).
    None,
    /// An activity entry, or a stride or travel-variant change, whose state
    /// the mesh can draw: the enemy is aiming on this tick.
    Aiming,
    /// Such a draw onto a state the mesh cannot draw: the switch fails and
    /// the mesh keeps what it shows, so there is no pose to read.
    Fails,
}

/// The interim aiming rule at the launch (`entity_model.md` §7c):
/// - no animation block: no pose;
/// - the airborne clip drawn at the launch: not aiming, through the landing tick;
/// - the brain's own draw landing this tick (an entry, a stride or
///   travel-variant change): aiming, posed where that draw starts the clip;
/// - a switch the brain did not make still drawn, or a pending redraw: not
///   aiming, even when this tick's own switch fails;
/// - a failed own switch, this tick or earlier, or a tracked state of none:
///   no pose;
/// - otherwise aiming on the tracked state at the clock's time.
pub(crate) fn classify(
    mesh: Option<&MeshComponent>,
    clock: &ClipClock,
    interruption: Interruption,
    airborne_drawn: bool,
    own_draw: OwnDraw,
    start_time: f64,
) -> Aim {
    let Some(animation) = mesh.and_then(|mesh| mesh.animation.as_ref()) else {
        return Aim::NoPose;
    };
    if airborne_drawn {
        return Aim::NotAiming;
    }
    if own_draw == OwnDraw::Aiming {
        // The draw starts the tracked state's clip over when the mesh shows
        // another state (a reaction over a state the entry shares) or a fresh
        // mesh awaits its redraw: pose where it starts, so the shot leaves the
        // drawn gun, not the time the clock kept meanwhile.
        let restarts = clock.redraw
            || clock
                .state
                .as_deref()
                .is_some_and(|state| animation.current_state != state);
        return match clock.state {
            None => Aim::NoPose,
            Some(_) if restarts => Aim::Aiming { time: start_time },
            Some(_) => Aim::Aiming { time: clock.time },
        };
    }
    if interruption == Interruption::Foreign || clock.redraw {
        return Aim::NotAiming;
    }
    if own_draw == OwnDraw::Fails || interruption == Interruption::FailedOwnSwitch {
        return Aim::NoPose;
    }
    match clock.state {
        None => Aim::NoPose,
        Some(_) => Aim::Aiming { time: clock.time },
    }
}

/// The world and content an NPC launch reads, borrowed for one AI apply.
#[derive(Clone, Copy, Default)]
pub(crate) struct LaunchInputs<'a> {
    /// World and movers at this tick's pose, as NPC sight sees them. `None`
    /// tests no near path, as a missing world sees everything.
    pub world: Option<&'a CombinedCollisionWorld<'a>>,
    pub descriptors: &'a [EntityTypeDescriptor],
    /// Every loaded model; `None` poses no muzzle.
    pub store: Option<&'a HitZoneStore>,
}

/// What the launch does with one due shot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ShotLaunch {
    /// Launch along this aim: from the muzzle, or from the eye for a missing
    /// muzzle or no pose.
    Launch(WeaponAim),
    /// The near-path test held it: cover close to the gun.
    Held,
    /// The enemy is not aiming.
    NotAiming,
}

/// One NPC's launch inputs for this tick's apply, read before the weapon
/// advances so no registry borrow is held across it.
pub(crate) struct NpcShooter<'a> {
    pub aim: Aim,
    /// The eye-origin aim: today's origin, the fallback for a missing muzzle.
    pub eye_aim: WeaponAim,
    pub eye: Vec3,
    pub target_aim: Vec3,
    pub world: Option<&'a CombinedCollisionWorld<'a>>,
    /// Each candidate weapon's posed muzzle, `None` where it is missing.
    pub muzzles: Vec<(String, Option<Vec3>)>,
}

impl NpcShooter<'_> {
    /// Decide one due shot of `weapon`, whose projectile sweeps at `radius`.
    pub(crate) fn decide(&self, weapon: &str, radius: f32) -> ShotLaunch {
        if self.aim == Aim::NotAiming {
            return ShotLaunch::NotAiming;
        }
        let muzzle = match self.aim {
            Aim::Aiming { .. } => self
                .muzzles
                .iter()
                .find(|(name, _)| name == weapon)
                .and_then(|(_, muzzle)| *muzzle),
            _ => None,
        };
        // A missing muzzle is not a blocked one: only the eye gate holds it.
        let Some(muzzle) = muzzle else {
            return ShotLaunch::Launch(self.eye_aim);
        };
        if let Some(world) = self.world
            && near_path_blocked(world, self.eye, muzzle, self.target_aim, radius)
        {
            return ShotLaunch::Held;
        }
        match (self.target_aim - muzzle).try_normalize() {
            Some(direction) => ShotLaunch::Launch(WeaponAim {
                origin: muzzle,
                direction,
            }),
            // A target aim point at the muzzle: fly as the eye aims.
            None => ShotLaunch::Launch(WeaponAim {
                origin: muzzle,
                direction: self.eye_aim.direction,
            }),
        }
    }
}

/// The two near paths at the projectile's radius, world and movers at this
/// tick's pose: body (eye) to muzzle, then muzzle toward the target's aim
/// point out to [`NEAR_PATH_DISTANCE_M`], or the whole way to a nearer aim
/// point, stopping one radius short of it: a surface the shot would only
/// graze beside the aim point does not stand between the gun and a target
/// at point blank, as sight forgives a hit at its aim point. Past the near
/// distance the shot flies as fired.
pub(crate) fn near_path_blocked(
    world: &CombinedCollisionWorld<'_>,
    eye: Vec3,
    muzzle: Vec3,
    target_aim: Vec3,
    radius: f32,
) -> bool {
    if leg_blocked(world, eye, muzzle, radius) {
        return true;
    }
    let toward = target_aim - muzzle;
    let reach = (toward.length() - radius.max(0.0)).clamp(0.0, NEAR_PATH_DISTANCE_M);
    match toward.try_normalize() {
        Some(direction) => leg_blocked(world, muzzle, muzzle + direction * reach, radius),
        None => false,
    }
}

fn leg_blocked(world: &CombinedCollisionWorld<'_>, from: Vec3, to: Vec3, radius: f32) -> bool {
    let delta = to - from;
    let length = delta.length();
    if length <= DEGENERATE_LEG_M {
        return false;
    }
    if radius > 0.0 {
        cast_sphere_exact_combined(world, from, radius, delta / length, length).is_some()
    } else {
        !line_of_sight_combined(from, to, world)
    }
}

/// Pose each candidate weapon's muzzle for `actor` at `time` on the clock's
/// tracked state, at the facing and position its transform holds now (the
/// facing apply wrote this tick, the tick-start position). A weapon with no
/// muzzle to read maps to `None`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pose_muzzles(
    registry: &EntityRegistry,
    actor: EntityId,
    clock: &ClipClock,
    time: f64,
    weapons: &[&str],
    descriptors: &[EntityTypeDescriptor],
    store: Option<&HitZoneStore>,
    scratch: &mut Vec<Mat4>,
) -> Vec<(String, Option<Vec3>)> {
    let posed = |weapon: &str, scratch: &mut Vec<Mat4>| -> Option<Vec3> {
        let store = store?;
        let state = clock.state.as_deref()?;
        let provenance = registry.get_component::<DescriptorProvenance>(actor).ok()?;
        let descriptor = descriptors.iter().find(|descriptor| {
            descriptor.canonical_name.as_deref() == Some(&provenance.canonical_name)
        })?;
        let mount = npc_muzzle_mount(descriptor, weapon, store)?;
        let transform = registry.get_component::<Transform>(actor).ok()?;
        let mesh = registry.get_component::<MeshComponent>(actor).ok()?;
        let model_to_world = model_matrix(transform, mesh.origin_offset, mesh.body_frame)?;
        posed_muzzle_world(
            store,
            model_to_world,
            mesh,
            ClipPose { state, time },
            mount,
            scratch,
        )
    };
    weapons
        .iter()
        .map(|weapon| (weapon.to_string(), posed(weapon, scratch)))
        .collect()
}

#[cfg(test)]
#[path = "npc_launch_tests.rs"]
mod tests;
