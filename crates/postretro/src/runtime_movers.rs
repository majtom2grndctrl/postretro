//! Runtime consumption of PRL kinematic mover records.
//!
//! This module owns the load-spawn and collision-collider feed for section 43.
//! Network registration is owned by netcode/lifecycle, not this module.

use std::collections::{HashMap, HashSet};
use std::fmt;

use glam::{Quat, Vec3};
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, KinematicMoverComponent,
    KinematicMoverMode, Transform,
};
use postretro_level_loader::{
    KinematicGeometry, LevelWorld, LoadedKinematicMover, LoadedKinematicWaypoint,
};
use postretro_visibility::VisibleCells;

use crate::collision::moving::MoverCollider;
use crate::render::{KinematicMoverInstance, MoverOccluderAabb};

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

pub(crate) struct KinematicMoverRenderCollector {
    /// Camera-PVS-visible instances for the beauty pass.
    instances: Vec<KinematicMoverInstance>,
    /// Renderable movers sent to shadow-depth recording.
    shadow_instances: Vec<KinematicMoverInstance>,
    occluder_aabbs: Vec<MoverOccluderAabb>,
    mover_bounds: HashMap<u32, postretro_render_data::cone_frustum::Aabb>,
    mover_bounds_source: Option<MoverBoundsSource>,
    visible_cell_bounds: Vec<(u32, Vec3, Vec3)>,
}

impl KinematicMoverRenderCollector {
    pub(crate) fn new() -> Self {
        Self {
            instances: Vec::new(),
            shadow_instances: Vec::new(),
            occluder_aabbs: Vec::new(),
            mover_bounds: HashMap::new(),
            mover_bounds_source: None,
            visible_cell_bounds: Vec::new(),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.instances.clear();
        self.shadow_instances.clear();
        self.occluder_aabbs.clear();
        self.mover_bounds.clear();
        self.mover_bounds_source = None;
        self.visible_cell_bounds.clear();
    }

    pub(crate) fn collect(
        &mut self,
        registry: &EntityRegistry,
        world: &LevelWorld,
        visible: &VisibleCells,
        alpha: f32,
    ) {
        self.instances.clear();
        self.shadow_instances.clear();
        self.occluder_aabbs.clear();
        self.refresh_mover_bounds(world);
        self.rebuild_visible_cell_bounds(world, visible);

        for (id, value) in registry.iter_with_kind(ComponentKind::KinematicMover) {
            let ComponentValue::KinematicMover(mover) = value else {
                continue;
            };
            let Ok(current) = registry.get_component::<Transform>(id) else {
                continue;
            };
            let Some(local_bounds) = self.mover_bounds.get(&mover.mover_id).copied() else {
                continue;
            };

            let current_model = transform_matrix(*current);
            let current_world_aabb = local_bounds.transformed(&current_model);
            let interpolated = registry
                .interpolated_transform(id, alpha)
                .unwrap_or(*current);
            let transform = transform_matrix(interpolated);
            let world_aabb = local_bounds.transformed(&transform);
            let instance = KinematicMoverInstance {
                mover_id: mover.mover_id,
                transform,
            };
            // Shadow cones intentionally ignore camera PVS. A mover outside the
            // camera-visible cells can still shadow a visible receiver.
            self.shadow_instances.push(instance);
            self.occluder_aabbs.push(MoverOccluderAabb {
                mover_id: mover.mover_id,
                world_aabb,
            });

            let origin_cell = world.locate_cell(current.position) as u32;
            if mover_visible_against_cell_bounds(
                visible,
                origin_cell,
                current_world_aabb.min,
                current_world_aabb.max,
                &self.visible_cell_bounds,
            ) {
                self.instances.push(instance);
            }
        }
    }

    pub(crate) fn instances(&self) -> &[KinematicMoverInstance] {
        &self.instances
    }

    /// Interpolated transforms for renderable movers sent to shadow-depth recording.
    pub(crate) fn shadow_instances(&self) -> &[KinematicMoverInstance] {
        &self.shadow_instances
    }

    /// World bounds for renderable movers used by promotion and depth culling.
    pub(crate) fn occluder_aabbs(&self) -> &[MoverOccluderAabb] {
        &self.occluder_aabbs
    }

    fn refresh_mover_bounds(&mut self, world: &LevelWorld) {
        let source = MoverBoundsSource::from_movers(&world.kinematic_geometry.movers);
        if self.mover_bounds_source == Some(source) {
            return;
        }

        self.mover_bounds.clear();
        self.mover_bounds
            .reserve(world.kinematic_geometry.movers.len());
        for mover in &world.kinematic_geometry.movers {
            if let Some(bounds) = mover_local_bounds(mover) {
                self.mover_bounds.insert(mover.mover_id, bounds);
            }
        }
        self.mover_bounds_source = Some(source);
    }

    fn rebuild_visible_cell_bounds(&mut self, world: &LevelWorld, visible: &VisibleCells) {
        self.visible_cell_bounds.clear();
        let VisibleCells::Culled(cells) = visible else {
            return;
        };

        self.visible_cell_bounds.reserve(cells.len());
        self.visible_cell_bounds
            .extend(cells.iter().filter_map(|cell| {
                world
                    .cell_bounds(*cell as usize)
                    .map(|(min, max)| (*cell, min, max))
            }));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MoverBoundsSource {
    ptr: usize,
    len: usize,
}

impl MoverBoundsSource {
    fn from_movers(movers: &[LoadedKinematicMover]) -> Self {
        Self {
            ptr: movers.as_ptr() as usize,
            len: movers.len(),
        }
    }
}

fn spawn_from_geometry(
    registry: &mut EntityRegistry,
    geometry: &KinematicGeometry,
) -> Result<Vec<EntityId>, RuntimeMoverLoadError> {
    let waypoint_indices = waypoint_index_map(&geometry.waypoints)?;
    let mut spawned = Vec::with_capacity(geometry.movers.len());

    for mover in &geometry.movers {
        let spin_axis = mover.spin_axis.normalize_or_zero();
        let has_initial_spin = mover.spin_speed_deg_s != 0.0;
        if has_initial_spin && spin_axis == Vec3::ZERO {
            return Err(RuntimeMoverLoadError::new(format!(
                "mover {} (`{}`) has nonzero spin_speed_deg_s but a zero spin_axis",
                mover.mover_id, mover.name
            )));
        }
        let allow_single_waypoint = has_initial_spin;
        let (waypoints, waypoint_names) = resolve_waypoint_chain(
            mover,
            &geometry.waypoints,
            &waypoint_indices,
            allow_single_waypoint,
        )?;
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
            waypoint_names,
            mover.speed_mps,
            mover.wait_ms,
            mode,
            mover.start_on_spawn,
            spin_axis,
            mover.spin_speed_deg_s.to_radians(),
            mover.spin_accel_deg_s2.to_radians(),
            mover.carry_yaw,
        );
        log::info!("{}", kinematic_mover_load_summary(mover, &component));
        registry
            .set_component(entity, component)
            .map_err(|err| RuntimeMoverLoadError::new(err.to_string()))?;
        spawned.push(entity);
    }

    Ok(spawned)
}

/// Author-readable static and seeded-phase diagnostics emitted during level
/// install. Rates are converted back to the FGD's degrees-per-second units.
fn kinematic_mover_load_summary(
    loaded: &LoadedKinematicMover,
    component: &KinematicMoverComponent,
) -> String {
    let axis = component.spin_axis;
    format!(
        "[Loader] kinematic mover {} (`{}`): spin static(axis=[{:.3}, {:.3}, {:.3}], accel={:.2} deg/s², carry_yaw={}); phase(current_rate={:.2} deg/s, target_rate={:.2} deg/s)",
        loaded.mover_id,
        loaded.name,
        axis.x,
        axis.y,
        axis.z,
        component.spin_accel_rad_s2.to_degrees(),
        component.carry_yaw,
        component.spin_rate_rad_s.to_degrees(),
        component.spin_target_rate_rad_s.to_degrees(),
    )
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

fn mover_local_bounds(
    mover: &LoadedKinematicMover,
) -> Option<postretro_render_data::cone_frustum::Aabb> {
    if mover.indices.is_empty() {
        return None;
    }
    let first = mover.vertices.first()?;
    let mut min = Vec3::from(first.position);
    let mut max = min;
    for vertex in &mover.vertices[1..] {
        let pos = Vec3::from(vertex.position);
        min = min.min(pos);
        max = max.max(pos);
    }
    if !min.is_finite() || !max.is_finite() || min.x > max.x || min.y > max.y || min.z > max.z {
        return None;
    }
    Some(postretro_render_data::cone_frustum::Aabb { min, max })
}

fn transform_matrix(transform: Transform) -> glam::Mat4 {
    glam::Mat4::from_scale_rotation_translation(
        transform.scale,
        transform.rotation,
        transform.position,
    )
}

fn mover_visible_against_cell_bounds(
    visible: &VisibleCells,
    origin_cell: u32,
    world_min: Vec3,
    world_max: Vec3,
    visible_cell_bounds: &[(u32, Vec3, Vec3)],
) -> bool {
    match visible {
        VisibleCells::DrawAll => true,
        VisibleCells::Culled(cells) => {
            if cells.contains(&origin_cell) {
                return true;
            }
            visible_cell_bounds
                .iter()
                .any(|(_, min, max)| aabb_intersects(world_min, world_max, *min, *max))
        }
    }
}

fn aabb_intersects(a_min: Vec3, a_max: Vec3, b_min: Vec3, b_max: Vec3) -> bool {
    a_min.x <= b_max.x
        && a_max.x >= b_min.x
        && a_min.y <= b_max.y
        && a_max.y >= b_min.y
        && a_min.z <= b_max.z
        && a_max.z >= b_min.z
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
    allow_single_waypoint: bool,
) -> Result<(Vec<Vec3>, Vec<String>), RuntimeMoverLoadError> {
    if mover.path.is_empty() {
        return Err(RuntimeMoverLoadError::new(format!(
            "mover {} (`{}`) has an empty path",
            mover.mover_id, mover.name
        )));
    }

    let mut resolved = Vec::new();
    let mut resolved_names = Vec::new();
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
        resolved_names.push(waypoint.name.clone());
        if waypoint.next.is_empty() {
            break;
        }
        current = waypoint.next.as_str();
    }

    if resolved.len() < 2 && !allow_single_waypoint {
        return Err(RuntimeMoverLoadError::new(format!(
            "mover {} (`{}`) path `{}` resolves to {} waypoint(s); at least 2 required",
            mover.mover_id,
            mover.name,
            mover.path,
            resolved.len()
        )));
    }

    Ok((resolved, resolved_names))
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
    use postretro_level_loader::{
        CellData, CellLocatorChild, LevelWorld, LoadedKinematicMover, LoadedKinematicWaypoint,
    };

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
            spin_axis: Vec3::ZERO,
            spin_speed_deg_s: 0.0,
            spin_accel_deg_s2: 0.0,
            carry_yaw: false,
        }
    }

    #[test]
    fn load_summary_separates_static_spin_fields_from_seeded_phase_in_author_units() {
        let mut loaded = mover(1);
        loaded.name = "carousel".to_string();
        loaded.spin_axis = Vec3::new(0.0, 0.0, 3.0);
        loaded.spin_speed_deg_s = -90.0;
        loaded.spin_accel_deg_s2 = 45.0;
        loaded.carry_yaw = true;
        let component = KinematicMoverComponent::new(
            loaded.mover_id,
            vec![loaded.origin],
            vec!["carousel".to_string()],
            loaded.speed_mps,
            loaded.wait_ms,
            KinematicMoverMode::PingPong,
            loaded.start_on_spawn,
            loaded.spin_axis,
            loaded.spin_speed_deg_s.to_radians(),
            loaded.spin_accel_deg_s2.to_radians(),
            loaded.carry_yaw,
        );

        let summary = kinematic_mover_load_summary(&loaded, &component);

        assert!(summary.contains("spin static(axis=[0.000, 0.000, 1.000]"));
        assert!(summary.contains("accel=45.00 deg/s²"));
        assert!(summary.contains("carry_yaw=true"));
        assert!(summary.contains("phase(current_rate=-90.00 deg/s, target_rate=-90.00 deg/s)"));
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

    fn single_cell_world(kinematic_geometry: KinematicGeometry) -> LevelWorld {
        let mut world = LevelWorld::new_visibility_only(
            vec![CellData {
                bounds_min: Vec3::splat(-1.0e6),
                bounds_max: Vec3::splat(1.0e6),
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
        .expect("single-cell mover fixture must be visibility-valid");
        world.kinematic_geometry = kinematic_geometry;
        world
    }

    fn two_cell_world(kinematic_geometry: KinematicGeometry) -> LevelWorld {
        let mut world = LevelWorld::new_visibility_only(
            vec![
                CellData {
                    bounds_min: Vec3::splat(-10.0),
                    bounds_max: Vec3::splat(5.0),
                    face_start: 0,
                    face_count: 0,
                    portal_ref_start: 0,
                    portal_ref_count: 0,
                    is_solid: false,
                    is_exterior: false,
                    is_drawable: false,
                },
                CellData {
                    bounds_min: Vec3::splat(10.0),
                    bounds_max: Vec3::splat(20.0),
                    face_start: 0,
                    face_count: 0,
                    portal_ref_start: 0,
                    portal_ref_count: 0,
                    is_solid: false,
                    is_exterior: false,
                    is_drawable: false,
                },
            ],
            Vec::new(),
            CellLocatorChild::Cell(0),
            Vec::new(),
            Vec::new(),
            false,
        )
        .expect("two-cell mover fixture must be visibility-valid");
        world.kinematic_geometry = kinematic_geometry;
        world
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
        assert_eq!(mover.waypoint_names, ["a", "b"]);
        assert_eq!(mover.speed_mps, 2.0);
        assert_eq!(mover.wait_ms, 125.0);
        assert_eq!(mover.spin_axis, Vec3::ZERO);
        assert_eq!(mover.spin_rate_rad_s, 0.0);
        assert_eq!(mover.spin_target_rate_rad_s, 0.0);
        assert_eq!(mover.spin_accel_rad_s2, 0.0);
        assert!(!mover.carry_yaw);
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
        let err = spawn_from_geometry(&mut EntityRegistry::new(), &short).unwrap_err();
        assert!(err.to_string().contains("at least 2 required"));
    }

    #[test]
    fn pure_rotator_spawns_with_one_waypoint_and_ticks_without_completing() {
        let mut pure_rotator = geometry(0);
        pure_rotator.waypoints.truncate(1);
        pure_rotator.waypoints[0].next.clear();
        pure_rotator.movers[0].spin_axis = Vec3::new(0.0, 3.0, 4.0);
        pure_rotator.movers[0].spin_speed_deg_s = 90.0;
        pure_rotator.movers[0].spin_accel_deg_s2 = 180.0;
        pure_rotator.movers[0].carry_yaw = true;

        let mut registry = EntityRegistry::new();
        let id = spawn_from_geometry(&mut registry, &pure_rotator).unwrap()[0];
        let mover = registry
            .get_component::<KinematicMoverComponent>(id)
            .expect("pure rotator must receive a mover component");
        assert_eq!(mover.waypoints, vec![Vec3::new(1.0, 2.0, 3.0)]);
        assert_eq!(mover.waypoint_names, ["a"]);
        assert!((mover.spin_axis - Vec3::new(0.0, 0.6, 0.8)).length() <= 1.0e-6);
        assert!((mover.spin_rate_rad_s - std::f32::consts::FRAC_PI_2).abs() <= 1.0e-6);
        assert!((mover.spin_target_rate_rad_s - std::f32::consts::FRAC_PI_2).abs() <= 1.0e-6);
        assert!((mover.spin_accel_rad_s2 - std::f32::consts::PI).abs() <= 1.0e-6);
        assert!(mover.carry_yaw);
        assert!(mover.spin_angle_rad.abs() <= 1.0e-6);

        let mut table = crate::kinematic_mover::MoverTickStateTable::default();
        crate::kinematic_mover::run_kinematic_mover_tick(&mut registry, &mut table, 0.5);

        let mover = registry
            .get_component::<KinematicMoverComponent>(id)
            .expect("pure rotator component must remain installed");
        assert!(!mover.completed);
        let transform = registry
            .get_component::<Transform>(id)
            .expect("pure rotator transform must remain installed");
        assert!((transform.position - Vec3::new(1.0, 2.0, 3.0)).length() <= 1.0e-6);
        let expected_rotation =
            Quat::from_axis_angle(Vec3::new(0.0, 0.6, 0.8), std::f32::consts::FRAC_PI_4);
        assert!((transform.rotation.dot(expected_rotation).abs() - 1.0).abs() <= 1.0e-6);
    }

    #[test]
    fn spawning_nonzero_spin_with_zero_axis_fails_clearly() {
        let mut invalid = geometry(0);
        invalid.movers[0].spin_speed_deg_s = 90.0;

        let err = spawn_from_geometry(&mut EntityRegistry::new(), &invalid).unwrap_err();

        assert!(
            err.to_string()
                .contains("nonzero spin_speed_deg_s but a zero spin_axis")
        );
    }

    #[test]
    fn zero_initial_spin_preserves_authored_axis_for_later_spin_up() {
        let mut delayed_spin = geometry(0);
        delayed_spin.movers[0].spin_axis = Vec3::new(0.0, 2.0, 0.0);
        delayed_spin.movers[0].spin_accel_deg_s2 = 180.0;

        let mut registry = EntityRegistry::new();
        let id = spawn_from_geometry(&mut registry, &delayed_spin).unwrap()[0];
        let mover = registry
            .get_component::<KinematicMoverComponent>(id)
            .expect("delayed spin mover must receive a mover component");

        assert!((mover.spin_axis - Vec3::Y).length() <= 1.0e-6);
        assert!(mover.spin_rate_rad_s.abs() <= 1.0e-6);
        assert!(mover.spin_target_rate_rad_s.abs() <= 1.0e-6);
        assert!((mover.spin_accel_rad_s2 - std::f32::consts::PI).abs() <= 1.0e-6);
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
    fn render_collector_clear_invalidates_level_cache_state() {
        let mut collector = KinematicMoverRenderCollector::new();
        collector.instances.push(KinematicMoverInstance {
            mover_id: 7,
            transform: glam::Mat4::IDENTITY,
        });
        collector.shadow_instances.push(KinematicMoverInstance {
            mover_id: 7,
            transform: glam::Mat4::IDENTITY,
        });
        collector.occluder_aabbs.push(MoverOccluderAabb {
            mover_id: 7,
            world_aabb: postretro_render_data::cone_frustum::Aabb {
                min: Vec3::ZERO,
                max: Vec3::ONE,
            },
        });
        collector.mover_bounds.insert(
            7,
            postretro_render_data::cone_frustum::Aabb {
                min: Vec3::ZERO,
                max: Vec3::ONE,
            },
        );
        collector.mover_bounds_source = Some(MoverBoundsSource { ptr: 1, len: 1 });
        collector
            .visible_cell_bounds
            .push((2, Vec3::ZERO, Vec3::ONE));

        collector.clear();

        assert!(collector.instances.is_empty());
        assert!(collector.shadow_instances.is_empty());
        assert!(collector.occluder_aabbs.is_empty());
        assert!(collector.mover_bounds.is_empty());
        assert_eq!(collector.mover_bounds_source, None);
        assert!(collector.visible_cell_bounds.is_empty());
    }

    #[test]
    fn render_collector_exposes_interpolated_active_mover_occluder_aabb() {
        let mut registry = EntityRegistry::new();
        let geometry = geometry(0);
        let mover_id = spawn_from_geometry(&mut registry, &geometry).unwrap()[0];
        let world = single_cell_world(geometry);

        registry.snapshot_transforms();
        registry
            .set_component(
                mover_id,
                Transform {
                    position: Vec3::new(5.0, 2.0, 3.0),
                    ..Transform::default()
                },
            )
            .unwrap();

        let mut collector = KinematicMoverRenderCollector::new();
        collector.collect(&registry, &world, &VisibleCells::DrawAll, 0.5);

        assert_eq!(collector.instances().len(), 1);
        assert_eq!(collector.shadow_instances().len(), 1);
        assert_eq!(collector.occluder_aabbs().len(), 1);
        let aabb = collector.occluder_aabbs()[0];
        assert_eq!(aabb.mover_id, 7);
        const EPSILON: f32 = 1.0e-5;
        assert!(
            (aabb.world_aabb.min - Vec3::new(3.0, 2.0, 3.0))
                .abs()
                .max_element()
                <= EPSILON,
            "interpolated mover AABB min must match the interpolated transform",
        );
        assert!(
            (aabb.world_aabb.max - Vec3::new(4.0, 3.0, 3.0))
                .abs()
                .max_element()
                <= EPSILON,
            "interpolated mover AABB max must match the interpolated transform",
        );
    }

    #[test]
    fn render_collector_retains_offscreen_caster_while_culling_beauty_draw() {
        // Regression: camera-PVS culling used to drop this mover from the
        // shadow path, even when its light cone could reach a visible receiver.
        let mut registry = EntityRegistry::new();
        let geometry = geometry(0);
        spawn_from_geometry(&mut registry, &geometry).unwrap();
        let world = two_cell_world(geometry);

        let mut collector = KinematicMoverRenderCollector::new();
        collector.collect(&registry, &world, &VisibleCells::Culled(vec![1]), 0.0);

        assert!(collector.instances().is_empty());
        assert_eq!(collector.shadow_instances().len(), 1);
        assert_eq!(collector.occluder_aabbs().len(), 1);
        assert_eq!(collector.shadow_instances()[0].mover_id, 7);
        assert_eq!(collector.occluder_aabbs()[0].mover_id, 7);
    }

    #[test]
    fn render_collector_replaces_shadow_casters_when_movers_are_absent() {
        let mut registry = EntityRegistry::new();
        let valid_geometry = geometry(0);
        spawn_from_geometry(&mut registry, &valid_geometry).unwrap();
        let world = single_cell_world(valid_geometry);
        let mut collector = KinematicMoverRenderCollector::new();

        collector.collect(&registry, &world, &VisibleCells::DrawAll, 0.0);
        collector.collect(&EntityRegistry::new(), &world, &VisibleCells::DrawAll, 0.0);

        assert!(collector.instances().is_empty());
        assert!(collector.shadow_instances().is_empty());
        assert!(collector.occluder_aabbs().is_empty());
    }

    #[test]
    fn render_collector_excludes_geometry_less_movers_from_shadow_and_promotion_sets() {
        // Regression: an empty mover fell back to a zero AABB and could consume
        // a promoted static-light slot despite having no installed geometry.
        let mut registry = EntityRegistry::new();
        let valid_geometry = geometry(0);
        spawn_from_geometry(&mut registry, &valid_geometry).unwrap();
        let world = single_cell_world(valid_geometry);
        let mut collector = KinematicMoverRenderCollector::new();

        collector.collect(&registry, &world, &VisibleCells::DrawAll, 0.0);
        assert_eq!(collector.shadow_instances().len(), 1);
        assert_eq!(collector.occluder_aabbs().len(), 1);

        let mut geometry_without_draws = geometry(0);
        geometry_without_draws.movers[0].vertices.clear();
        let world_without_draws = single_cell_world(geometry_without_draws);
        collector.collect(&registry, &world_without_draws, &VisibleCells::DrawAll, 0.0);

        assert!(collector.instances().is_empty());
        assert!(collector.shadow_instances().is_empty());
        assert!(collector.occluder_aabbs().is_empty());
    }

    #[test]
    fn mover_culling_keeps_origin_in_visible_cell() {
        let visible = VisibleCells::Culled(vec![2]);
        assert!(mover_visible_against_cell_bounds(
            &visible,
            2,
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(11.0, 1.0, 1.0),
            &[],
        ));
    }

    #[test]
    fn mover_culling_keeps_aabb_overlapping_visible_cell_even_when_origin_is_elsewhere() {
        let visible = VisibleCells::Culled(vec![2]);
        assert!(mover_visible_against_cell_bounds(
            &visible,
            9,
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(2.0, 2.0, 2.0),
            &[(2, Vec3::ZERO, Vec3::ONE)],
        ));
    }

    #[test]
    fn mover_culling_drops_non_overlapping_non_visible_mover() {
        let visible = VisibleCells::Culled(vec![2]);
        assert!(!mover_visible_against_cell_bounds(
            &visible,
            9,
            Vec3::new(4.0, 4.0, 4.0),
            Vec3::new(5.0, 5.0, 5.0),
            &[(2, Vec3::ZERO, Vec3::ONE)],
        ));
    }

    #[test]
    fn render_collector_transforms_rotated_mover_bounds() {
        let mut registry = EntityRegistry::new();
        let geometry = geometry(0);
        let mover_id = spawn_from_geometry(&mut registry, &geometry).unwrap()[0];
        registry
            .set_component(
                mover_id,
                Transform {
                    position: Vec3::ZERO,
                    rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
                    scale: Vec3::ONE,
                },
            )
            .unwrap();

        let world = single_cell_world(geometry);
        let mut collector = KinematicMoverRenderCollector::new();
        collector.collect(&registry, &world, &VisibleCells::DrawAll, 1.0);

        let aabb = collector.occluder_aabbs()[0].world_aabb;
        const EPSILON: f32 = 1.0e-5;
        assert!((aabb.min - Vec3::new(0.0, 0.0, -1.0)).abs().max_element() <= EPSILON);
        assert!((aabb.max - Vec3::new(0.0, 1.0, 0.0)).abs().max_element() <= EPSILON);
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
