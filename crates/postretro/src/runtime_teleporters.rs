// Level-installed teleporter entities and movement bindings.
// See: context/lib/entity_model.md §7 · context/lib/movement.md §6

use glam::Vec3;
use postretro_entities::{
    ComponentKind, ComponentValue, EntityRegistry, TeleporterComponent, Transform,
};
use postretro_level_format::teleporters::TELEPORTER_SOURCE_ENTITY_KEY;
use postretro_level_loader::LevelWorld;

pub(crate) fn spawn_loaded_teleporters(registry: &mut EntityRegistry, world: &LevelWorld) {
    for (index, record) in world.teleporters.teleporters.iter().enumerate() {
        let companion = world.map_entities.iter().enumerate().find(|(_, entity)| {
            entity.classname == "trigger_teleport"
                && entity.key_values.iter().any(|(key, value)| {
                    key == TELEPORTER_SOURCE_ENTITY_KEY
                        && value.parse::<u32>().ok() == Some(record.source_entity)
                })
        });
        let position = companion.map_or_else(
            || {
                record.brushes.first().map_or(Vec3::ZERO, |brush| {
                    (Vec3::from(brush.aabb_min) + Vec3::from(brush.aabb_max)) * 0.5
                })
            },
            |(_, entity)| Vec3::from(entity.origin),
        );
        let tags = companion.map_or(&[][..], |(_, entity)| entity.tags.as_slice());
        let Some(entity) = registry.try_spawn(
            Transform {
                position,
                ..Default::default()
            },
            tags,
        ) else {
            log::warn!(
                "[Loader] entity registry exhausted; dropping teleporter entity {}",
                record.source_entity
            );
            break;
        };
        let _ = registry.set_component(
            entity,
            TeleporterComponent {
                index: index as u16,
                source_entity: record.source_entity,
                armed: record.enabled_on_spawn,
                arrival_sound: record.arrival_sound.clone(),
                on_teleport: record.on_teleport.clone(),
            },
        );
        if let Some((ordinal, _)) = companion {
            let _ = registry.set_component(
                entity,
                postretro_entities::components::map_ordinal::MapOrdinal(ordinal as u32),
            );
        }
    }
}

pub(crate) fn bind_loaded_teleporters(
    world: &mut postretro_physics::teleporters::Teleporters,
    registry: &EntityRegistry,
) {
    for (id, value) in registry.iter_with_kind(ComponentKind::Teleporter) {
        if let ComponentValue::Teleporter(teleporter) = value {
            world.bind_entity(usize::from(teleporter.index), id);
        }
    }
}

pub(crate) fn local_teleport_identity(registry: &EntityRegistry) -> Option<(u8, f32)> {
    let pawn = registry.local_player_movement_pawn()?;
    let movement = registry
        .get_component::<postretro_foundation::PlayerMovementComponent>(pawn)
        .ok()?;
    Some((
        movement.teleport.generation,
        movement.teleport.accumulated_yaw,
    ))
}

pub(crate) fn hash_content(hasher: &mut blake3::Hasher, world: &LevelWorld) {
    use postretro_level_format::teleporters::{TeleportMode, TeleporterRecord};
    let count = world.teleporters.teleporters.len() as u64;
    hasher.update(&count.to_le_bytes());
    for record in &world.teleporters.teleporters {
        let TeleporterRecord {
            source_entity,
            mode,
            enabled_on_spawn: _,
            brushes,
            destination_origin,
            destination_yaw,
            landmark_origin,
            landmark_yaw,
            arrival_sound: _,
            on_teleport: _,
        } = record;
        hasher.update(&source_entity.to_le_bytes());
        hasher.update(&[match mode {
            TeleportMode::Classic => 0,
            TeleportMode::Seamless => 1,
        }]);
        hasher.update(&(brushes.len() as u64).to_le_bytes());
        for brush in brushes {
            for value in brush.aabb_min.iter().chain(&brush.aabb_max) {
                hasher.update(&value.to_le_bytes());
            }
            hasher.update(&(brush.planes.len() as u64).to_le_bytes());
            for plane in &brush.planes {
                for value in plane.normal {
                    hasher.update(&value.to_le_bytes());
                }
                hasher.update(&plane.distance.to_le_bytes());
            }
        }
        for value in destination_origin {
            hasher.update(&value.to_le_bytes());
        }
        hasher.update(&destination_yaw.to_le_bytes());
        // Classic landmark fields are ignored by the loader and movement.
        if matches!(mode, TeleportMode::Seamless) {
            for value in landmark_origin {
                hasher.update(&value.to_le_bytes());
            }
            hasher.update(&landmark_yaw.to_le_bytes());
        }
        let companion = world.map_entities.iter().find(|entity| {
            entity.classname == "trigger_teleport"
                && entity.key_values.iter().any(|(key, value)| {
                    key == TELEPORTER_SOURCE_ENTITY_KEY
                        && value.parse::<u32>().ok() == Some(*source_entity)
                })
        });
        for key in ["target", "landmark"] {
            let value = companion
                .and_then(|entity| entity.key_values.iter().find(|(name, _)| name == key))
                .map_or("", |(_, value)| value.as_str());
            hasher.update(&(value.len() as u64).to_le_bytes());
            hasher.update(value.as_bytes());
        }
    }
}

pub(crate) fn apply_local_camera_turn(
    camera: &mut crate::camera::Camera,
    registry: &EntityRegistry,
    before: Option<(u8, f32)>,
) {
    if let (Some((_, before)), Some((_, after))) = (before, local_teleport_identity(registry)) {
        camera.yaw += postretro_foundation::wrap_teleport_yaw(after - before);
    }
}

/// Hold the camera at the arrival eye and detach the local view from mover carry.
/// The carry ground is the only cached carry state: the next input seam's yaw carry
/// and the render-only yaw residual (`effective_render_yaw`) both derive from it, so
/// airborne here removes the platform's rotation at tick rate and at every alpha.
pub(crate) fn hold_local_arrival(
    camera: &crate::camera::Camera,
    timing: &mut crate::frame_timing::FrameTiming,
    carry: &mut postretro_foundation::GroundRef,
) {
    *carry = postretro_foundation::GroundRef::Airborne;
    timing.hold_state(crate::frame_timing::InterpolableState::new(camera.position));
}

/// A connected client's forward-predicted teleport on this tick: turn the view by
/// the predicted delta, detach mover carry, and start the traveller cue now. Reconcile
/// replay never reaches here; it reports through [`apply_local_adoptions`].
pub(crate) fn apply_predicted_teleport(
    camera: &mut crate::camera::Camera,
    carry: &mut postretro_foundation::GroundRef,
    sounds: &mut Vec<postretro_audio::SoundRequest>,
    teleporters: &postretro_physics::teleporters::Teleporters,
    event: &postretro_physics::movement::TeleportEvent,
) {
    camera.yaw += event.yaw_delta;
    *carry = postretro_foundation::GroundRef::Airborne;
    sounds.extend(crate::sound_events::arrival_sound(
        teleporters,
        event.teleporter,
        event.arrival,
    ));
}

/// The host seat's own traveller cue from one authoritative tick. Remote pawns'
/// teleports reach the host seat as an observer, through
/// [`host_observed_arrival_cues`].
pub(crate) fn push_local_traveller_cues(
    sounds: &mut Vec<postretro_audio::SoundRequest>,
    teleporters: &postretro_physics::teleporters::Teleporters,
    local_pawn: Option<postretro_entities::EntityId>,
    teleports: &[(
        postretro_entities::EntityId,
        postretro_physics::movement::TeleportEvent,
    )],
) {
    for (pawn, event) in teleports {
        if Some(*pawn) == local_pawn {
            sounds.extend(crate::sound_events::arrival_sound(
                teleporters,
                event.teleporter,
                event.arrival,
            ));
        }
    }
}

/// Host-seat observer cues for connected-client pawns. Drain once after the frame's
/// authoritative ticks and before the audio step: each pawn's generation changes
/// since the last drain coalesce to its last authoritative sample, at that sample's
/// pose, whatever delayed pose the host still presents. No-op off the host.
pub(crate) fn host_observed_arrival_cues(
    endpoint: Option<&mut postretro_netcode::NetEndpoint>,
    teleporters: &postretro_physics::teleporters::Teleporters,
    sounds: &mut Vec<postretro_audio::SoundRequest>,
) {
    let Some(postretro_netcode::NetEndpoint::Host {
        client_pawn_presentation,
        owners,
        allocator,
        ..
    }) = endpoint
    else {
        return;
    };
    let observed = postretro_netcode::host_take_client_pawn_teleports(
        client_pawn_presentation,
        owners,
        allocator,
    );
    crate::sound_events::observed_arrival_sounds(teleporters, &observed, sounds);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_local_adoptions(
    camera: &mut crate::camera::Camera,
    timing: &mut crate::frame_timing::FrameTiming,
    carry: &mut postretro_foundation::GroundRef,
    sounds: &mut Vec<postretro_audio::SoundRequest>,
    teleporters: &postretro_physics::teleporters::Teleporters,
    registry: &EntityRegistry,
    adoptions: &[postretro_netcode::LocalTeleportAdoption],
) {
    for adoption in adoptions {
        camera.yaw += adoption.yaw_delta;
        if adoption.camera_cut {
            if let (Ok(pose), Ok(movement)) = (
                registry.get_component::<Transform>(adoption.entity_id),
                registry.get_component::<postretro_foundation::PlayerMovementComponent>(
                    adoption.entity_id,
                ),
            ) {
                camera.position = pose.position + Vec3::Y * movement.capsule.eye_height;
            }
            hold_local_arrival(camera, timing, carry);
        }
        if let Some(index) = adoption.cue_teleporter {
            sounds.extend(crate::sound_events::arrival_sound(
                teleporters,
                index,
                adoption.arrival,
            ));
        }
    }
}

#[cfg(test)]
mod app_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
    use postretro_level_format::map_entity::MapEntityRecord;
    use postretro_level_format::teleporters::{TeleportMode, TeleporterRecord};
    use postretro_level_loader::CellLocatorChild;

    fn world() -> LevelWorld {
        let mut world = LevelWorld::new_visibility_only(
            vec![postretro_level_loader::CellData {
                bounds_min: Vec3::splat(-100.0),
                bounds_max: Vec3::splat(100.0),
                face_start: 0,
                face_count: 0,
                portal_ref_start: 0,
                portal_ref_count: 0,
                is_solid: false,
                is_exterior: false,
                is_drawable: false,
            }],
            Vec::new(),
            CellLocatorChild::Cell(0),
            Vec::new(),
            Vec::new(),
            false,
        )
        .unwrap();
        let planes = (0..3)
            .flat_map(|axis| {
                let mut normal = [0.0; 3];
                normal[axis] = 1.0;
                let mut negative = normal;
                negative[axis] = -1.0;
                [
                    EnvironmentPlane {
                        normal,
                        distance: 1.0,
                    },
                    EnvironmentPlane {
                        normal: negative,
                        distance: 1.0,
                    },
                ]
            })
            .collect();
        world.teleporters.teleporters.push(TeleporterRecord {
            source_entity: 1,
            mode: TeleportMode::Seamless,
            enabled_on_spawn: false,
            brushes: vec![EnvironmentBrush {
                aabb_min: [-1.0; 3],
                aabb_max: [1.0; 3],
                planes,
            }],
            destination_origin: [20.0, 2.0, 0.0],
            destination_yaw: 1.0,
            landmark_origin: [0.0; 3],
            landmark_yaw: 0.0,
            arrival_sound: "teleport".into(),
            on_teleport: "used".into(),
        });
        world.map_entities.push(MapEntityRecord {
            classname: "trigger_teleport".into(),
            origin: [0.0; 3],
            angles: [0.0; 3],
            tags: vec!["gate".into()],
            key_values: vec![
                (TELEPORTER_SOURCE_ENTITY_KEY.into(), "1".into()),
                ("target".into(), "room_b".into()),
                ("landmark".into(), "room_a".into()),
            ],
        });
        world
    }
    fn digest(world: &LevelWorld) -> blake3::Hash {
        let mut h = blake3::Hasher::new();
        hash_content(&mut h, world);
        h.finalize()
    }

    #[test]
    fn teleport_content_digest_includes_prediction_fields_and_pairing_only() {
        let base = world();
        let expected = digest(&base);
        for edit in [
            |r: &mut TeleporterRecord| r.source_entity += 1,
            |r: &mut TeleporterRecord| r.mode = TeleportMode::Classic,
            |r: &mut TeleporterRecord| r.brushes[0].planes[0].distance += 0.1,
            |r: &mut TeleporterRecord| r.brushes[0].aabb_max[0] += 0.1,
            |r: &mut TeleporterRecord| r.destination_origin[0] += 0.1,
            |r: &mut TeleporterRecord| r.destination_yaw += 0.1,
            |r: &mut TeleporterRecord| r.landmark_origin[1] += 0.1,
            |r: &mut TeleporterRecord| r.landmark_yaw += 0.1,
        ] {
            let mut changed = world();
            edit(&mut changed.teleporters.teleporters[0]);
            assert_ne!(digest(&changed), expected);
        }
        let mut presentation = world();
        let r = &mut presentation.teleporters.teleporters[0];
        r.arrival_sound = "other".into();
        r.enabled_on_spawn = true;
        r.on_teleport = "other.used".into();
        assert_eq!(digest(&presentation), expected);
        let mut changed = world();
        changed.map_entities[0].key_values[1].1 = "identical_pose_other_name".into();
        assert_ne!(digest(&changed), expected);
        let mut changed = world();
        changed.map_entities[0].tags.push("other".into());
        assert_eq!(digest(&changed), expected);
    }

    #[test]
    fn teleport_install_preserves_tags_ordinal_and_armed_binding() {
        let mut world = world();
        world.map_entities[0].origin = [0.5, 0.0, 0.0];
        let mut registry = EntityRegistry::new();
        spawn_loaded_teleporters(&mut registry, &world);
        let mut physical =
            postretro_physics::teleporters::Teleporters::from_section(&world.teleporters);
        bind_loaded_teleporters(&mut physical, &registry);
        let entity = physical.entity(0).unwrap();
        assert_eq!(registry.get_tags(entity).unwrap(), &["gate".to_owned()]);
        assert!(
            (registry
                .get_component::<Transform>(entity)
                .unwrap()
                .position
                - Vec3::new(0.5, 0.0, 0.0))
            .length()
                < 0.0001
        );
        assert_eq!(
            registry
                .get_component::<postretro_entities::components::map_ordinal::MapOrdinal>(entity)
                .unwrap()
                .0,
            0
        );
        assert!(!physical.armed(&registry, 0));
        let mut state = registry
            .get_component::<TeleporterComponent>(entity)
            .unwrap()
            .clone();
        state.armed = true;
        registry.set_component(entity, state).unwrap();
        assert!(physical.armed(&registry, 0));
    }
    #[test]
    fn teleport_reconcile_camera_hold_keeps_arrival_eye_at_every_alpha() {
        use crate::frame_timing::{FrameTiming, InterpolableState};
        let world = world();
        let physical =
            postretro_physics::teleporters::Teleporters::from_section(&world.teleporters);
        let mut registry = EntityRegistry::new();
        let arrival = Vec3::new(20.0, 2.0, 0.0);
        let pawn = registry.spawn(Transform {
            position: arrival,
            ..Default::default()
        });
        let movement = postretro_foundation::PlayerMovementComponent::from_descriptor(
            &crate::mod_digest::tests::movement_descriptor(),
        );
        let eye = arrival + Vec3::Y * movement.capsule.eye_height;
        registry.set_component(pawn, movement).unwrap();
        registry.mark_local_player_pawn(pawn).unwrap();
        let mut camera = crate::camera::Camera::new(Vec3::ZERO, 0.2, 0.3);
        let mut timing = FrameTiming::new(InterpolableState::new(Vec3::ZERO));
        let mut carry = postretro_foundation::GroundRef::Mover(7);
        let mut sounds = Vec::new();
        apply_local_adoptions(
            &mut camera,
            &mut timing,
            &mut carry,
            &mut sounds,
            &physical,
            &registry,
            &[postretro_netcode::LocalTeleportAdoption {
                entity_id: pawn,
                yaw_delta: 1.0,
                camera_cut: true,
                arrival,
                cue_teleporter: Some(0),
            }],
        );
        assert!((camera.yaw - 1.2).abs() < 0.0001);
        assert!((camera.pitch - 0.3).abs() < 0.0001);
        assert_eq!(carry, postretro_foundation::GroundRef::Airborne);
        for alpha in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let shown = timing.previous_state.lerp(&timing.current_state, alpha);
            assert!((shown.position - eye).length() < 0.0001);
        }
        timing.push_state(InterpolableState::new(eye + Vec3::X * 0.1));
        assert!((timing.previous_state.position - eye).length() < 0.0001);
        assert_eq!(sounds.len(), 1);
        assert_eq!(
            sounds[0].anchor,
            Some(postretro_audio::SoundAnchor::Point(arrival.to_array()))
        );
    }

    #[test]
    fn teleport_host_camera_and_fire_command_use_arrival_eye_and_rotated_view() {
        let mut level = world();
        level.teleporters.teleporters[0].mode = TeleportMode::Classic;
        level.teleporters.teleporters[0].enabled_on_spawn = true;
        level.teleporters.teleporters[0].destination_yaw = std::f32::consts::FRAC_PI_2;
        let collision = crate::collision::CollisionWorld::from_level(&level).unwrap();
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let movement = postretro_foundation::PlayerMovementComponent::from_descriptor(
            &crate::mod_digest::tests::movement_descriptor(),
        );
        registry.set_component(pawn, movement).unwrap();
        registry.mark_local_player_pawn(pawn).unwrap();
        let before = local_teleport_identity(&registry);
        let mut camera = crate::camera::Camera::new(Vec3::ZERO, 0.0, 0.3);
        let command = crate::build_sim_command(
            &crate::input::ActionSnapshot::neutral(),
            &camera,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
        );
        let events = postretro_sim::sim::run_host_movement_tick(
            &mut registry,
            &collision,
            postretro_collision::EnvironmentView::level_only(0.0),
            &[(pawn, command.movement)],
            1.0 / 60.0,
        );
        assert!(events[0].1.teleport.is_some());
        apply_local_camera_turn(&mut camera, &registry, before);
        crate::follow_camera_to_local_pawn(&mut camera, &registry, Vec3::ZERO);
        let shot = crate::build_post_movement_command(&camera);
        assert!((shot.aim_origin - camera.position).length() < 0.0001);
        let expected = Vec3::new(-0.3_f32.cos(), 0.3_f32.sin(), 0.0);
        assert!((shot.aim_direction - expected).length() < 0.0001);
        assert!((camera.position.x - 20.0).abs() < 0.0001);
    }
    #[test]
    fn teleport_catchup_camera_and_mesh_cut_on_first_middle_last_and_zero_tick_frames() {
        use crate::frame_timing::{FrameTiming, InterpolableState};
        for teleport_tick in 0..3 {
            let mut level = world();
            let record = &mut level.teleporters.teleporters[0];
            record.mode = TeleportMode::Classic;
            record.enabled_on_spawn = true;
            record.destination_yaw = std::f32::consts::FRAC_PI_2;
            let collision = crate::collision::CollisionWorld::from_level(&level).unwrap();
            let mut registry = EntityRegistry::new();
            let start = Vec3::new(-1.0 - 0.1 * (teleport_tick as f32 + 0.5), 0.0, 0.0);
            let pawn = registry.spawn(Transform {
                position: start,
                ..Default::default()
            });
            let mut movement = postretro_foundation::PlayerMovementComponent::from_descriptor(
                &crate::mod_digest::tests::movement_descriptor(),
            );
            movement.velocity = Vec3::X * 6.0;
            let eye_height = movement.capsule.eye_height;
            registry.set_component(pawn, movement).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();
            let mut camera = crate::camera::Camera::new(start + Vec3::Y * eye_height, 0.0, 0.0);
            let mut timing = FrameTiming::new(InterpolableState::new(camera.position));
            let mut carry = postretro_foundation::GroundRef::Mover(7);
            let mut cuts = 0;
            for tick in 0..3 {
                let before = local_teleport_identity(&registry);
                let command = crate::build_sim_command(
                    &crate::input::ActionSnapshot::neutral(),
                    &camera,
                    false,
                    false,
                    false,
                    false,
                    false,
                    false,
                    false,
                );
                registry.snapshot_transforms();
                let events = postretro_sim::sim::run_host_movement_tick(
                    &mut registry,
                    &collision,
                    postretro_collision::EnvironmentView::level_only(0.0),
                    &[(pawn, command.movement)],
                    1.0 / 60.0,
                );
                apply_local_camera_turn(&mut camera, &registry, before);
                crate::follow_camera_to_local_pawn(&mut camera, &registry, Vec3::ZERO);
                if events[0].1.teleport.is_some() {
                    assert_eq!(tick, teleport_tick);
                    cuts += 1;
                    hold_local_arrival(&camera, &mut timing, &mut carry);
                } else {
                    timing.push_state(InterpolableState::new(camera.position));
                }
            }
            assert_eq!(cuts, 1);
            assert_eq!(carry, postretro_foundation::GroundRef::Airborne);
            // The final zero-tick frame performs no further push or snapshot.
            for alpha in [0.0, 0.25, 0.5, 0.75, 1.0] {
                assert!(
                    (timing
                        .previous_state
                        .lerp(&timing.current_state, alpha)
                        .position
                        .x
                        - 20.0)
                        .abs()
                        < 0.0001
                );
                assert!(
                    (registry
                        .interpolated_transform(pawn, alpha)
                        .unwrap()
                        .position
                        .x
                        - 20.0)
                        .abs()
                        < 0.0001
                );
            }
        }
    }
}
