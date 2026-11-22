// Rider foot probes against each mover's presented pose, the pose the rider is
// drawn on, once per frame on every seat.
// See: context/lib/networking.md §Host-side remote-pawn presentation

use std::collections::HashMap;

use postretro_entities::components::agent::AgentComponent;
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::{GroundRef, PlayerMovementComponent};

use crate::collision::CollisionWorld;
use crate::collision::moving::{MoverCollider, MoverPose, MoverPoseSource};
use crate::kinematic_mover::MoverTickStateTable;
use crate::scripting_systems::hit_zones::HitZoneStore;

use super::pose_inputs::write_foot_ground_probes;

/// Each mover's tick state with its transform replaced by the pose the frame
/// draws it at: the registry's presented pose at the frame alpha, smoothing
/// included. Refreshed once per frame; reuses its storage.
#[derive(Debug, Default)]
pub struct PresentedMoverPoses {
    poses: HashMap<u32, MoverPose>,
}

impl PresentedMoverPoses {
    pub fn refresh(&mut self, table: &MoverTickStateTable, registry: &EntityRegistry, alpha: f32) {
        self.poses.clear();
        for (mover_id, state) in table.iter() {
            let transform = registry
                .interpolated_transform(state.entity, alpha)
                .unwrap_or(state.transform);
            self.poses.insert(
                mover_id,
                MoverPose {
                    transform,
                    linear_velocity: state.linear_velocity,
                    tick_delta: state.tick_delta,
                    angular_velocity: state.angular_velocity,
                    tick_rotation_delta: state.tick_rotation_delta,
                    carry_yaw: state.carry_yaw,
                    tick_dt: state.tick_dt,
                },
            );
        }
    }
}

impl MoverPoseSource for PresentedMoverPoses {
    fn pose(&self, mover_id: u32) -> Option<MoverPose> {
        self.poses.get(&mover_id).copied()
    }
}

/// How a probe pass treats feet standing on a mover.
#[derive(Clone, Copy)]
pub enum RiderFeet<'a> {
    /// The fixed tick leaves riders to the per-frame pass
    /// ([`update_rider_foot_probes`]).
    Skip,
    /// Probe riders from their presented pose at `alpha` against `poses`.
    /// `remote_riders` adds remotes the frame placed on a mover, which carry
    /// no ground of their own on a connected client.
    Presented {
        poses: &'a dyn MoverPoseSource,
        alpha: f32,
        remote_riders: &'a [EntityId],
    },
}

/// Whether `id` stands on a mover: its own ground names one, or the frame
/// placed it on one.
pub(crate) fn rides_mover(
    registry: &EntityRegistry,
    id: EntityId,
    remote_riders: &[EntityId],
) -> bool {
    let on_mover = |ground: GroundRef| matches!(ground, GroundRef::Mover(_));
    registry
        .get_component::<PlayerMovementComponent>(id)
        .is_ok_and(|movement| on_mover(movement.ground))
        || registry
            .get_component::<AgentComponent>(id)
            .is_ok_and(|agent| on_mover(agent.ground))
        || remote_riders.contains(&id)
}

/// Probe one rider's feet from its presented pose against `poses`.
#[allow(clippy::too_many_arguments)] // mirrors the probe pass's inputs.
pub(crate) fn probe_rider(
    registry: &mut EntityRegistry,
    id: EntityId,
    collision_world: &CollisionWorld,
    mover_colliders: &[MoverCollider],
    poses: &dyn MoverPoseSource,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    alpha: f32,
) {
    let transform: Option<Transform> = registry.interpolated_transform(id, alpha).ok();
    write_foot_ground_probes(
        registry,
        id,
        transform,
        collision_world,
        mover_colliders,
        poses,
        hit_zone_store,
        anim_time,
    );
}

/// Single-player and host: probe every legged rider once per frame, from its
/// presented pose against each mover's presented pose. The fixed tick skips
/// them ([`RiderFeet::Skip`]); feet off a mover keep the per-tick probe.
pub fn update_rider_foot_probes(
    registry: &mut EntityRegistry,
    collision_world: &CollisionWorld,
    mover_colliders: &[MoverCollider],
    poses: &PresentedMoverPoses,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    alpha: f32,
) {
    let mut candidates = hit_zone_store.foot_probe_candidates();
    candidates.clear();
    candidates.extend(
        registry
            .iter_with_kind(postretro_entities::ComponentKind::Mesh)
            .filter_map(|(id, value)| {
                let postretro_entities::ComponentValue::Mesh(mesh) = value else {
                    return None;
                };
                super::pose_inputs::is_foot_probe_candidate(mesh, hit_zone_store).then_some(id)
            }),
    );
    candidates.retain(|id| rides_mover(registry, *id, &[]));
    for &id in candidates.iter() {
        probe_rider(
            registry,
            id,
            collision_world,
            mover_colliders,
            poses,
            hit_zone_store,
            anim_time,
            alpha,
        );
    }
}

#[cfg(test)]
#[path = "rider_feet_tests.rs"]
mod tests;
