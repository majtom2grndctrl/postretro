// Where an anchored sound is: an emitter's fire-time point, and its presented
// position each frame. Player pawns sound from the eye, movers from the center
// of their bounds, everything else from its origin.
// See: context/lib/audio.md §4 (Anchors)

use glam::Vec3;
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, KinematicMoverComponent, Transform,
};
use postretro_foundation::PlayerMovementComponent;
use postretro_level_loader::LevelWorld;

use crate::runtime_movers::KinematicMoverRenderCollector;

/// The audio boundary's opaque key for an entity. Generation bits ride along,
/// so a key never resolves to a later entity reusing the slot.
pub(crate) fn entity_key(id: EntityId) -> u64 {
    u64::from(id.to_raw())
}

fn entity_from_key(key: u64) -> Option<EntityId> {
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
    /// The emitter's point this tick, from its current (not interpolated) pose.
    pub(crate) fn fire_time_point(&mut self, id: EntityId) -> Option<Vec3> {
        let pose = *self.registry.get_component::<Transform>(id).ok()?;
        self.placed(id, pose)
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

/// Find a mover's entity by its level-stable mover id.
pub(super) fn mover_entity(registry: &EntityRegistry, mover_id: u32) -> Option<EntityId> {
    registry
        .iter_with_kind(ComponentKind::KinematicMover)
        .find_map(|(id, value)| match value {
            ComponentValue::KinematicMover(mover) if mover.mover_id == mover_id => Some(id),
            _ => None,
        })
}
