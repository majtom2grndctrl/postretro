//! Runtime consumption of PRL kinematic mover records.
//!
//! This module owns the load-spawn and collision-collider feed for section 43.
//! Network registration is intentionally absent until replication client-apply
//! can bind movers by `mover_id`.

use std::collections::{HashMap, HashSet};
use std::fmt;

use glam::{Quat, Vec3};
use postretro_entities::{
    EntityId, EntityRegistry, KinematicMoverComponent, KinematicMoverMode, Transform,
};
use postretro_level_loader::{
    KinematicGeometry, LevelWorld, LoadedKinematicMover, LoadedKinematicWaypoint,
};

use crate::collision::moving::MoverCollider;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeMoverLoadError {
    message: String,
}

impl RuntimeMoverLoadError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for RuntimeMoverLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RuntimeMoverLoadError {}

pub(crate) fn spawn_loaded_kinematic_movers(
    registry: &mut EntityRegistry,
    world: &LevelWorld,
) -> Result<Vec<EntityId>, RuntimeMoverLoadError> {
    spawn_from_geometry(registry, &world.kinematic_geometry)
}

pub(crate) fn build_loaded_mover_colliders(world: &LevelWorld) -> Vec<MoverCollider> {
    world
        .kinematic_geometry
        .movers
        .iter()
        .filter_map(build_mover_collider)
        .collect()
}

fn spawn_from_geometry(
    registry: &mut EntityRegistry,
    geometry: &KinematicGeometry,
) -> Result<Vec<EntityId>, RuntimeMoverLoadError> {
    let waypoint_indices = waypoint_index_map(&geometry.waypoints)?;
    let mut spawned = Vec::with_capacity(geometry.movers.len());

    for mover in &geometry.movers {
        let waypoints = resolve_waypoint_chain(mover, &geometry.waypoints, &waypoint_indices)?;
        let mode = mover_mode(mover)?;
        let transform = Transform {
            position: mover.origin,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        };
        let Some(entity) = registry.try_spawn(transform, &mover.tags) else {
            return Err(RuntimeMoverLoadError::new(format!(
                "entity registry exhausted while spawning mover {} (`{}`)",
                mover.mover_id, mover.name
            )));
        };
        let component = KinematicMoverComponent::new(
            mover.mover_id,
            waypoints,
            mover.speed_mps,
            mover.wait_ms,
            mode,
            mover.start_on_spawn,
        );
        registry
            .set_component(entity, component)
            .map_err(|err| RuntimeMoverLoadError::new(err.to_string()))?;
        spawned.push(entity);
    }

    Ok(spawned)
}

fn build_mover_collider(mover: &LoadedKinematicMover) -> Option<MoverCollider> {
    let vertices: Vec<Vec3> = mover
        .vertices
        .iter()
        .map(|vertex| Vec3::from(vertex.position))
        .collect();
    let triangles: Vec<[u32; 3]> = mover
        .indices
        .chunks_exact(3)
        .map(|chunk| [chunk[0], chunk[1], chunk[2]])
        .collect();
    MoverCollider::from_local_triangles(mover.mover_id, &vertices, &triangles)
}

fn waypoint_index_map(
    waypoints: &[LoadedKinematicWaypoint],
) -> Result<HashMap<&str, usize>, RuntimeMoverLoadError> {
    let mut indices = HashMap::with_capacity(waypoints.len());
    for (index, waypoint) in waypoints.iter().enumerate() {
        if indices.insert(waypoint.name.as_str(), index).is_some() {
            return Err(RuntimeMoverLoadError::new(format!(
                "duplicate waypoint name `{}`",
                waypoint.name
            )));
        }
    }
    Ok(indices)
}

fn resolve_waypoint_chain(
    mover: &LoadedKinematicMover,
    waypoints: &[LoadedKinematicWaypoint],
    waypoint_indices: &HashMap<&str, usize>,
) -> Result<Vec<Vec3>, RuntimeMoverLoadError> {
    if mover.path.is_empty() {
        return Err(RuntimeMoverLoadError::new(format!(
            "mover {} (`{}`) has an empty path",
            mover.mover_id, mover.name
        )));
    }

    let mut resolved = Vec::new();
    let mut seen = HashSet::new();
    let mut current = mover.path.as_str();
    loop {
        if !seen.insert(current.to_string()) {
            return Err(RuntimeMoverLoadError::new(format!(
                "mover {} (`{}`) waypoint chain cycles at `{current}`",
                mover.mover_id, mover.name
            )));
        }
        let Some(&index) = waypoint_indices.get(current) else {
            return Err(RuntimeMoverLoadError::new(format!(
                "mover {} (`{}`) references unknown waypoint `{current}`",
                mover.mover_id, mover.name
            )));
        };
        let waypoint = &waypoints[index];
        resolved.push(waypoint.origin);
        if waypoint.next.is_empty() {
            break;
        }
        current = waypoint.next.as_str();
    }

    if resolved.len() < 2 {
        return Err(RuntimeMoverLoadError::new(format!(
            "mover {} (`{}`) path `{}` resolves to {} waypoint(s); at least 2 required",
            mover.mover_id,
            mover.name,
            mover.path,
            resolved.len()
        )));
    }

    Ok(resolved)
}

fn mover_mode(mover: &LoadedKinematicMover) -> Result<KinematicMoverMode, RuntimeMoverLoadError> {
    match mover.move_mode {
        0 => Ok(KinematicMoverMode::Once),
        1 => Ok(KinematicMoverMode::PingPong),
        other => Err(RuntimeMoverLoadError::new(format!(
            "mover {} (`{}`) has invalid move_mode {other}",
            mover.mover_id, mover.name
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::ComponentKind;
    use postretro_level_format::geometry::Vertex;
    use postretro_level_loader::{LoadedKinematicMover, LoadedKinematicWaypoint};

    fn vertex(position: [f32; 3]) -> Vertex {
        Vertex::new(
            position,
            [0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        )
    }

    fn mover(mode: u8) -> LoadedKinematicMover {
        LoadedKinematicMover {
            mover_id: 7,
            name: "lift".to_string(),
            tags: vec!["platform".to_string(), "arena".to_string()],
            origin: Vec3::new(1.0, 2.0, 3.0),
            path: "a".to_string(),
            speed_mps: 2.0,
            wait_ms: 125.0,
            move_mode: mode,
            start_on_spawn: true,
            vertices: vec![
                vertex([0.0, 0.0, 0.0]),
                vertex([1.0, 0.0, 0.0]),
                vertex([0.0, 1.0, 0.0]),
            ],
            indices: vec![0, 1, 2],
            face_meta: Vec::new(),
        }
    }

    fn geometry(mode: u8) -> KinematicGeometry {
        KinematicGeometry {
            movers: vec![mover(mode)],
            waypoints: vec![
                LoadedKinematicWaypoint {
                    name: "a".to_string(),
                    next: "b".to_string(),
                    origin: Vec3::new(1.0, 2.0, 3.0),
                },
                LoadedKinematicWaypoint {
                    name: "b".to_string(),
                    next: String::new(),
                    origin: Vec3::new(3.0, 2.0, 3.0),
                },
            ],
        }
    }

    #[test]
    fn spawn_loaded_movers_creates_transform_component_tags_and_resolved_waypoints() {
        let mut registry = EntityRegistry::new();
        let spawned = spawn_from_geometry(&mut registry, &geometry(1)).unwrap();

        assert_eq!(spawned.len(), 1);
        let id = spawned[0];
        let transform = registry.get_component::<Transform>(id).unwrap();
        assert_eq!(transform.position, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(
            registry.get_tags(id).unwrap(),
            &["platform".to_string(), "arena".to_string()]
        );

        let mover = registry
            .get_component::<KinematicMoverComponent>(id)
            .unwrap();
        assert_eq!(mover.mover_id, 7);
        assert_eq!(mover.mode, KinematicMoverMode::PingPong);
        assert_eq!(
            mover.waypoints,
            vec![Vec3::new(1.0, 2.0, 3.0), Vec3::new(3.0, 2.0, 3.0)]
        );
        assert_eq!(mover.speed_mps, 2.0);
        assert_eq!(mover.wait_ms, 125.0);
        assert!(mover.started);
        assert!(matches!(
            registry.has_component_kind(id, ComponentKind::KinematicMover),
            Ok(true)
        ));
    }

    #[test]
    fn invalid_waypoint_chain_is_rejected_consistently() {
        let mut bad = geometry(0);
        bad.waypoints[0].next = "missing".to_string();
        assert!(spawn_from_geometry(&mut EntityRegistry::new(), &bad).is_err());

        let mut cycle = geometry(0);
        cycle.waypoints[1].next = "a".to_string();
        assert!(spawn_from_geometry(&mut EntityRegistry::new(), &cycle).is_err());

        let mut short = geometry(0);
        short.waypoints.truncate(1);
        short.waypoints[0].next.clear();
        assert!(spawn_from_geometry(&mut EntityRegistry::new(), &short).is_err());
    }

    #[test]
    fn loaded_mover_collision_geometry_builds_through_mover_collider() {
        let geometry = geometry(0);
        let colliders: Vec<_> = geometry
            .movers
            .iter()
            .filter_map(build_mover_collider)
            .collect();

        assert_eq!(colliders.len(), 1);
        assert_eq!(colliders[0].mover_id, 7);
    }

    #[test]
    fn fixed_tick_advances_loaded_once_and_ping_pong_movers() {
        for (mode, expected_after_first, expected_after_second, completed) in [
            (0, Vec3::new(2.0, 2.0, 3.0), Vec3::new(3.0, 2.0, 3.0), true),
            (1, Vec3::new(2.0, 2.0, 3.0), Vec3::new(3.0, 2.0, 3.0), false),
        ] {
            let mut registry = EntityRegistry::new();
            let mut geometry = geometry(mode);
            geometry.movers[0].wait_ms = 0.0;
            let spawned = spawn_from_geometry(&mut registry, &geometry).unwrap();
            let mut table = crate::kinematic_mover::MoverTickStateTable::default();

            crate::kinematic_mover::run_kinematic_mover_tick(&mut registry, &mut table, 0.5);
            let transform = registry.get_component::<Transform>(spawned[0]).unwrap();
            assert!((transform.position - expected_after_first).length() <= 1.0e-5);

            crate::kinematic_mover::run_kinematic_mover_tick(&mut registry, &mut table, 0.5);
            let transform = registry.get_component::<Transform>(spawned[0]).unwrap();
            let mover = registry
                .get_component::<KinematicMoverComponent>(spawned[0])
                .unwrap();
            assert!((transform.position - expected_after_second).length() <= 1.0e-5);
            assert_eq!(mover.completed, completed);
        }
    }
}
