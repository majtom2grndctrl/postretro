// Host-local mover transition edges resolved to their authored event address,
// sound key, and anchor. One edge per crush victim, so two victims sound twice.
// See: context/lib/audio.md §4 · context/lib/entity_model.md §7

use glam::Vec3;
use postretro_entities::{EntityId, KinematicMoverComponent};
use postretro_physics::kinematic_mover::MoverEventKind;

use super::anchors::{AnchorScene, entity_key, mover_entity};

/// One mover edge as the app drain sees it. Either authored field may be
/// absent; an edge with neither is dropped.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MoverEdge {
    /// Named-reaction address the edge fires, from the mover's `*_event` KVP.
    pub(crate) address: Option<String>,
    /// Sound key the edge plays, from the mover's `*_sound` KVP.
    pub(crate) sound: Option<String>,
    /// The mover entity the sound follows.
    pub(crate) emitter: EntityId,
    /// Center of the mover's world bounds when the edge fired; `None` only for
    /// a mover with no transform, which fires its address but plays nothing.
    pub(crate) point: Option<Vec3>,
}

impl MoverEdge {
    /// The anchored SFX request for this edge's sound, if it names one.
    pub(crate) fn sound_request(&self) -> Option<postretro_audio::SoundRequest> {
        let sound = self.sound.clone()?;
        let point = self.point?;
        Some(postretro_audio::SoundRequest {
            bus: "sfx".to_string(),
            sound,
            looping: false,
            anchor: Some(postretro_audio::SoundAnchor::Entity {
                key: entity_key(self.emitter),
                point: point.to_array(),
            }),
        })
    }
}

/// Resolve each `(kind, mover_id)` edge against its source mover. Missing
/// movers and edges with no authored address or sound are ordinary no-ops.
pub(crate) fn resolve_mover_edges(
    events: &[(MoverEventKind, u32)],
    scene: &mut AnchorScene<'_>,
) -> Vec<MoverEdge> {
    events
        .iter()
        .filter_map(|(kind, mover_id)| {
            let emitter = mover_entity(scene.registry, *mover_id)?;
            let mover = scene
                .registry
                .get_component::<KinematicMoverComponent>(emitter)
                .ok()?;
            let address = kind.dispatch_address(mover).map(str::to_owned);
            let sound = kind.sound_key(mover).map(str::to_owned);
            if address.is_none() && sound.is_none() {
                return None;
            }
            let point = scene.fire_time_point(emitter);
            Some(MoverEdge {
                address,
                sound,
                emitter,
                point,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_movers::KinematicMoverRenderCollector;
    use crate::runtime_movers::tests::{mover, single_cell_world};
    use glam::Quat;
    use postretro_entities::{EntityRegistry, KinematicMoverConfig, KinematicMoverMode, Transform};
    use postretro_level_loader::KinematicGeometry;

    /// Mover 7 at x = 10, whose local bounds center is `[0.5, 0.5, 0]`.
    fn door(
        registry: &mut EntityRegistry,
        configure: impl FnOnce(&mut KinematicMoverComponent),
    ) -> EntityId {
        let entity = registry.spawn(Transform {
            position: Vec3::new(10.0, 0.0, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        });
        let mut component = KinematicMoverComponent::new(
            7,
            KinematicMoverConfig {
                waypoints: vec![Vec3::ZERO, Vec3::X],
                waypoint_names: vec!["closed".to_string(), "open".to_string()],
                speed_mps: 1.0,
                wait_ms: 0.0,
                mode: KinematicMoverMode::PingPong,
                started: true,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        );
        configure(&mut component);
        registry
            .set_component(entity, component)
            .expect("door component attaches");
        entity
    }

    fn resolve(registry: &EntityRegistry, events: &[(MoverEventKind, u32)]) -> Vec<MoverEdge> {
        let world = single_cell_world(KinematicGeometry {
            movers: vec![mover(1)],
            waypoints: Vec::new(),
        });
        let mut movers = KinematicMoverRenderCollector::new();
        let mut scene = AnchorScene {
            registry,
            world: Some(&world),
            movers: &mut movers,
        };
        resolve_mover_edges(events, &mut scene)
    }

    #[test]
    fn door_edge_requests_one_sfx_play_anchored_at_its_bounds_center() {
        let mut registry = EntityRegistry::new();
        let entity = door(&mut registry, |door| {
            door.open_event = Some("door.open".to_string());
            door.open_sound = Some("sfx/door_open".to_string());
        });

        let edges = resolve(&registry, &[(MoverEventKind::Opened, 7)]);
        let [edge] = edges.as_slice() else {
            panic!("one edge, got {edges:?}");
        };
        assert_eq!(edge.address.as_deref(), Some("door.open"));
        assert_eq!(
            edge.sound_request(),
            Some(postretro_audio::SoundRequest {
                bus: "sfx".to_string(),
                sound: "sfx/door_open".to_string(),
                looping: false,
                anchor: Some(postretro_audio::SoundAnchor::Entity {
                    key: entity_key(entity),
                    point: [10.5, 0.5, 0.0],
                }),
            }),
        );
    }

    #[test]
    fn each_mover_edge_kind_plays_its_own_sound_key() {
        let mut registry = EntityRegistry::new();
        door(&mut registry, |door| {
            door.open_sound = Some("open".to_string());
            door.close_sound = Some("close".to_string());
            door.blocked_sound = Some("blocked".to_string());
            door.crush_sound = Some("crush".to_string());
        });
        let sounds: Vec<String> = resolve(
            &registry,
            &[
                (MoverEventKind::Opened, 7),
                (MoverEventKind::Closed, 7),
                (MoverEventKind::Blocked, 7),
                (MoverEventKind::Crushed, 7),
            ],
        )
        .iter()
        .filter_map(MoverEdge::sound_request)
        .map(|request| request.sound)
        .collect();
        assert_eq!(sounds, ["open", "close", "blocked", "crush"]);
    }

    // Pin P11: two crush edges on one tick are two sounds.
    #[test]
    fn two_crush_edges_on_one_tick_request_two_sounds() {
        let mut registry = EntityRegistry::new();
        door(&mut registry, |door| {
            door.crush_sound = Some("crush".to_string())
        });
        let requests: Vec<_> = resolve(
            &registry,
            &[(MoverEventKind::Crushed, 7), (MoverEventKind::Crushed, 7)],
        )
        .iter()
        .filter_map(MoverEdge::sound_request)
        .collect();
        assert_eq!(requests.len(), 2);
    }

    #[test]
    fn edge_with_an_address_and_no_sound_fires_its_address_and_plays_nothing() {
        let mut registry = EntityRegistry::new();
        door(&mut registry, |door| {
            door.close_event = Some("door.close".to_string())
        });
        let edges = resolve(&registry, &[(MoverEventKind::Closed, 7)]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].address.as_deref(), Some("door.close"));
        assert_eq!(edges[0].sound_request(), None);
    }

    #[test]
    fn edge_with_neither_address_nor_sound_is_dropped() {
        let mut registry = EntityRegistry::new();
        door(&mut registry, |_| {});
        assert!(resolve(&registry, &[(MoverEventKind::Opened, 7)]).is_empty());
        assert!(resolve(&registry, &[(MoverEventKind::Opened, 99)]).is_empty());
    }

    #[test]
    fn presented_point_follows_the_interpolated_pose_and_ends_with_the_entity() {
        let world = single_cell_world(KinematicGeometry {
            movers: vec![mover(1)],
            waypoints: Vec::new(),
        });
        let mut registry = EntityRegistry::new();
        let entity = door(&mut registry, |_| {});
        registry
            .set_component(
                entity,
                Transform {
                    position: Vec3::new(20.0, 0.0, 0.0),
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
            )
            .expect("move the door");
        let key = entity_key(entity);
        let mut movers = KinematicMoverRenderCollector::new();
        {
            let mut scene = AnchorScene {
                registry: &registry,
                world: Some(&world),
                movers: &mut movers,
            };
            let halfway = scene.presented_point(key, 0.5).expect("door is live");
            assert!(
                Vec3::from(halfway).abs_diff_eq(Vec3::new(15.5, 0.5, 0.0), 1.0e-4),
                "interpolated between the last two poses, got {halfway:?}",
            );
        }
        registry.despawn(entity).expect("despawn the door");
        let mut scene = AnchorScene {
            registry: &registry,
            world: Some(&world),
            movers: &mut movers,
        };
        assert_eq!(
            scene.presented_point(key, 0.5),
            None,
            "a gone entity freezes its sound"
        );
    }
}
