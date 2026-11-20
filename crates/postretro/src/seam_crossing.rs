//! Level seam crossings: the unload harvest's per-seat arrivals, and each
//! seat's first spawn in the next level taking one.
//! See: context/lib/entity_model.md §7e · context/lib/boot_sequence.md §4

use std::collections::BTreeMap;

use glam::{EulerRot, Quat, Vec3};
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::{PlayerMovementComponent, Seat};
use postretro_physics::collision::moving::CombinedCollisionWorld;
use postretro_physics::movement::seam_arrival::{
    Landmark, LandmarkRelative, SeamArrivalPose, apply_seam_arrival,
};
use postretro_sim::placement_occupancy::PlacementOccupancy;
use postretro_sim::seam_stage::{SeamStage, pawn_capsule};

/// A seam level change's arrivals: the crossing seam's name and, for each
/// seat whose pawn was inside it at the harvest, its landmark-relative pose
/// and motion. It belongs to the load that carried the seam: the next unload,
/// suspend, and a failed or cancelled load clear it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SeamCrossing {
    pub(crate) seam: String,
    pub(crate) arrivals: BTreeMap<Seat, LandmarkRelative>,
}

impl SeamCrossing {
    /// A crossing no seat arrives by: a seam load whose occupants all left.
    pub(crate) fn empty(seam: &str) -> Self {
        Self {
            seam: seam.to_owned(),
            arrivals: BTreeMap::new(),
        }
    }
}

/// A seat's arrival through a destination seam: which seam, its landmark, and
/// the pose and motion to apply. Rides an unbacked spawn to a client pawn's
/// participation entry.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SeamSpawnArrival {
    pub(crate) seam: String,
    pub(crate) seam_index: u16,
    pub(crate) landmark: Landmark,
    pub(crate) motion: LandmarkRelative,
}

impl SeamSpawnArrival {
    /// The arrival of `motion` through the installed level's seam named
    /// `seam`, or `None` when that level has no such seam.
    pub(crate) fn resolve(stage: &SeamStage, seam: &str, motion: LandmarkRelative) -> Option<Self> {
        let seam_index = stage.index_by_name(seam)?;
        let landmark = stage.landmark(seam_index)?;
        Some(Self {
            seam: seam.to_owned(),
            seam_index,
            landmark: Landmark {
                origin: landmark.origin,
                yaw: landmark.yaw,
            },
            motion,
        })
    }

    pub(crate) fn body_point(&self) -> Vec3 {
        self.motion.body_point(self.landmark)
    }

    pub(crate) fn view_yaw(&self) -> f32 {
        self.motion.world_view_yaw(self.landmark)
    }

    /// Whether the arrival capsule fits: the static world and movers, then
    /// every live pawn but `arriving`. Tested at spawn, never at harvest.
    pub(crate) fn fits(
        &self,
        collision: &CombinedCollisionWorld<'_>,
        registry: &EntityRegistry,
        arriving: Option<EntityId>,
        spawn_capsule: &postretro_foundation::CapsuleParams,
    ) -> bool {
        let body = self.body_point();
        let capsule = self
            .motion
            .crouched_capsule
            .clone()
            .unwrap_or_else(|| spawn_capsule.clone());
        if !body.is_finite()
            || !self.view_yaw().is_finite()
            || !self.motion.view_pitch.is_finite()
            || !postretro_physics::collision::capsule_fits_combined(collision, body, &capsule)
        {
            return false;
        }
        let feet = body - Vec3::Y * (capsule.radius + capsule.half_height);
        !PlacementOccupancy::new(registry, arriving, std::iter::empty())
            .live_pawn_overlaps(feet, Some(&capsule))
    }
}

/// Read every seated pawn inside seam `seam` into the landmark's frame. The
/// local pawn's view is the host camera (`local_view`, yaw and pitch); a
/// client's is what its host-side pawn faces (`client_view`, given the seat
/// and the pawn's teleport-frame yaw), or with no resolved command its pawn's
/// yaw at pitch 0. A seat with no bound pawn, or whose pawn is outside, gets
/// none.
pub(crate) fn harvest_crossing(
    seam: &str,
    stage: &SeamStage,
    registry: &EntityRegistry,
    seats: &[Seat],
    local_view: (f32, f32),
    mut client_view: impl FnMut(Seat, f32) -> Option<(f32, f32)>,
) -> SeamCrossing {
    let mut crossing = SeamCrossing::empty(seam);
    let Some(index) = stage.index_by_name(seam) else {
        return crossing;
    };
    let Some(landmark) = stage.landmark(index) else {
        return crossing;
    };
    let landmark = Landmark {
        origin: landmark.origin,
        yaw: landmark.yaw,
    };
    let local = registry.local_player_pawn();
    for &seat in seats {
        let Some(pawn) = registry.pawn_for_seat(seat) else {
            continue;
        };
        let (Ok(movement), Ok(transform), Some(capsule)) = (
            registry.get_component::<PlayerMovementComponent>(pawn),
            registry.get_component::<Transform>(pawn),
            pawn_capsule(registry, pawn),
        ) else {
            continue;
        };
        if !stage.contains(index, capsule) {
            continue;
        }
        let (yaw, pitch) = if Some(pawn) == local {
            local_view
        } else {
            client_view(seat, movement.teleport.accumulated_yaw)
                .unwrap_or_else(|| (transform.rotation.to_euler(EulerRot::YXZ).0, 0.0))
        };
        crossing.arrivals.insert(
            seat,
            LandmarkRelative::capture(movement, capsule.center, yaw, pitch, landmark),
        );
    }
    crossing
}

/// Why a seat's arrival placed it at a player start instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArrivalFallback {
    /// The installed level holds no seam of the crossing's name.
    MissingSeam,
    /// The arrival capsule overlaps the world, a mover or a live pawn.
    Blocked,
}

/// Log the one warning a fallen-back arrival carries.
pub(crate) fn warn_fallback(seam: &str, fallback: ArrivalFallback, who: &str) {
    match fallback {
        ArrivalFallback::MissingSeam => log::warn!(
            "[Loader] seam `{seam}`: this level has no seam of that name; {who} arrives at a player start"
        ),
        ArrivalFallback::Blocked => {
            log::warn!("[Loader] seam `{seam}` arrival blocked; {who} arrives at a player start")
        }
    }
}

/// Move a just-spawned `pawn` from its player start to `arrival`, under the
/// teleport arrival rule. A missing seam or a blocked arrival leaves the
/// pawn at its start untouched.
pub(crate) fn arrive_pawn(
    registry: &mut EntityRegistry,
    collision: &CombinedCollisionWorld<'_>,
    stage: &SeamStage,
    seam: &str,
    pawn: EntityId,
    motion: &LandmarkRelative,
) -> Result<(u16, SeamArrivalPose), ArrivalFallback> {
    let arrival = SeamSpawnArrival::resolve(stage, seam, motion.clone())
        .ok_or(ArrivalFallback::MissingSeam)?;
    apply_spawn_arrival(registry, collision, pawn, &arrival).map(|pose| (arrival.seam_index, pose))
}

/// Apply a resolved arrival to a spawned pawn: the fit and live-pawn tests,
/// then pose, motion and view yaw.
pub(crate) fn apply_spawn_arrival(
    registry: &mut EntityRegistry,
    collision: &CombinedCollisionWorld<'_>,
    pawn: EntityId,
    arrival: &SeamSpawnArrival,
) -> Result<SeamArrivalPose, ArrivalFallback> {
    let Ok(mut movement) = registry
        .get_component::<PlayerMovementComponent>(pawn)
        .cloned()
    else {
        return Err(ArrivalFallback::Blocked);
    };
    if !arrival.fits(collision, registry, Some(pawn), &movement.capsule) {
        return Err(ArrivalFallback::Blocked);
    }
    let (pose, _) = apply_seam_arrival(
        &mut movement,
        collision,
        &arrival.motion,
        arrival.landmark,
        |index| collision.static_world.teleporters.armed(registry, index),
    )
    .map_err(|_| ArrivalFallback::Blocked)?;
    let mut transform = registry
        .get_component::<Transform>(pawn)
        .copied()
        .unwrap_or_default();
    transform.position = pose.body;
    transform.rotation = Quat::from_rotation_y(pose.view_yaw);
    let _ = registry.set_component(pawn, movement);
    let _ = registry.set_component(pawn, transform);
    Ok(pose)
}

#[cfg(test)]
#[path = "seam_crossing_tests.rs"]
mod tests;
