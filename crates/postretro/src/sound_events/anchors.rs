// Where an anchored sound is: an emitter's fire-time point, and its presented
// position each frame. Player pawns sound from the eye, movers from the center
// of their bounds, everything else from its origin.
// See: context/lib/audio.md §4 (Anchors)

use glam::Vec3;
use postretro_audio::SoundAnchor;
use postretro_entities::{Emitter, EntityId, EntityRegistry, KinematicMoverComponent, Transform};
use postretro_foundation::PlayerMovementComponent;
use postretro_level_loader::LevelWorld;

use crate::runtime_movers::KinematicMoverRenderCollector;

/// The audio boundary's opaque key for an entity. Generation bits ride along,
/// so a key never resolves to a later entity reusing the slot.
pub(crate) fn entity_key(id: EntityId) -> u64 {
    u64::from(id.to_raw())
}

pub(crate) fn entity_from_key(key: u64) -> Option<EntityId> {
    u32::try_from(key).ok().map(EntityId::from_raw)
}

/// Key of the pawn the listener is attached to: the pawn the camera follows.
pub(crate) fn listener_attached_key(registry: &EntityRegistry) -> Option<u64> {
    registry.local_player_movement_pawn().map(entity_key)
}

/// What anchor placement reads: the registry, plus the level geometry a mover's
/// bounds come from.
pub(crate) struct AnchorScene<'a> {
    pub(crate) registry: &'a EntityRegistry,
    pub(crate) world: Option<&'a LevelWorld>,
    pub(crate) movers: &'a mut KinematicMoverRenderCollector,
}

impl AnchorScene<'_> {
    /// The emitter's point this tick, from its current (not interpolated) pose,
    /// carried by any render-only correction it is drawn with, so a mover's
    /// sound starts on the deck the frame draws.
    pub(crate) fn fire_time_point(&mut self, id: EntityId) -> Option<Vec3> {
        let pose = *self.registry.get_component::<Transform>(id).ok()?;
        // A mover's glide is a whole-pose correction; another entity's render
        // offset (the local pawn's eye placement) is alpha-dependent and stays
        // out of the fire-time point.
        let pose = match self.registry.get_component::<KinematicMoverComponent>(id) {
            Ok(_) => self
                .registry
                .presentation_correction(id)
                .map_or(pose, |correction| correction.apply(&pose)),
            Err(_) => pose,
        };
        self.placed(id, pose)
    }

    /// The audio anchor for an emitter, placed at fire time. An entity gone
    /// before the drain falls back to the origin its emission captured.
    pub(crate) fn sound_anchor(&mut self, emitter: &Emitter) -> SoundAnchor {
        match emitter {
            Emitter::Entity { id, origin } => SoundAnchor::Entity {
                key: entity_key(*id),
                point: self.fire_time_point(*id).unwrap_or(*origin).to_array(),
            },
            Emitter::Contacts(contacts) => SoundAnchor::Contacts(
                contacts
                    .iter()
                    .map(|contact| contact.point.to_array())
                    .collect(),
            ),
        }
    }

    /// Where the frame presents the emitter: its render-interpolated pose.
    /// `None` once the entity is gone, which freezes a playing sound.
    pub(crate) fn presented_point(&mut self, key: u64, alpha: f32) -> Option<[f32; 3]> {
        let id = entity_from_key(key)?;
        let pose = self.registry.interpolated_transform(id, alpha).ok()?;
        self.placed(id, pose).map(|point| point.to_array())
    }

    fn placed(&mut self, id: EntityId, pose: Transform) -> Option<Vec3> {
        if let Ok(mover) = self.registry.get_component::<KinematicMoverComponent>(id) {
            // A mover with no loaded geometry has no bounds; its origin is the
            // best remaining answer.
            let center = self
                .world
                .and_then(|world| self.movers.world_bounds_center(world, mover.mover_id, pose));
            return Some(center.unwrap_or(pose.position));
        }
        if let Ok(movement) = self.registry.get_component::<PlayerMovementComponent>(id) {
            return Some(pose.position + Vec3::new(0.0, movement.capsule.eye_height, 0.0));
        }
        Some(pose.position)
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;
    use postretro_entities::{
        EntityRegistry, KinematicMoverComponent, KinematicMoverConfig, KinematicMoverMode,
        Transform,
    };

    use super::AnchorScene;
    use crate::runtime_movers::KinematicMoverRenderCollector;

    fn at(x: f32) -> Transform {
        Transform {
            position: Vec3::new(x, 0.0, 0.0),
            ..Transform::default()
        }
    }

    // A sound fired from a mover gliding onto a correction starts on the deck
    // the frame draws, not on the corrected pose the deck has yet to reach.
    #[test]
    fn mover_sound_fires_at_the_drawn_pose_while_a_correction_glides() {
        let mut registry = EntityRegistry::new();
        let mover = registry.spawn(at(2.0));
        registry
            .set_component(
                mover,
                KinematicMoverComponent::new(
                    7,
                    KinematicMoverConfig {
                        waypoints: vec![Vec3::ZERO],
                        waypoint_names: vec!["a".into()],
                        speed_mps: 0.0,
                        wait_ms: 0.0,
                        mode: KinematicMoverMode::Once,
                        started: false,
                        spin_axis: Vec3::ZERO,
                        initial_spin_rate_rad_s: 0.0,
                        spin_accel_rad_s2: 0.0,
                        carry_yaw: false,
                    },
                ),
            )
            .unwrap();
        registry.correct_transform(mover, at(2.5), true).unwrap();
        let mut movers = KinematicMoverRenderCollector::new();
        let mut scene = AnchorScene {
            registry: &registry,
            world: None,
            movers: &mut movers,
        };
        let fired = scene.fire_time_point(mover).unwrap();
        let drawn = scene
            .presented_point(super::entity_key(mover), 1.0)
            .unwrap();
        assert!(
            (fired - Vec3::new(2.0, 0.0, 0.0)).length() < 1.0e-6,
            "{fired}"
        );
        assert_eq!(fired.to_array(), drawn);
    }
}
