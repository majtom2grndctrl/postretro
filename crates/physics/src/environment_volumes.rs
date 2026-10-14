// Adapter from loaded EnvironmentVolumes records (PRL id 52) to the collision crate's query set.
// See: context/lib/entity_model.md §7 (Environment Volumes) · context/lib/build_pipeline.md §PRL section IDs (EnvironmentVolumes)

use glam::Vec3;
use postretro_collision::{ConvexLeaf, EnvironmentVolume, EnvironmentVolumes, LeafPlane};
use postretro_level_loader::LevelWorld;

/// Build the level's environment volume set from its loaded records, keeping
/// map order (the overlap rule's last tiebreak). `resolve_fluid` maps a
/// record's fluid name to the session fluid index; a name it does not know, and
/// an empty name, load dry. Each volume takes its record's force from the
/// optional PushForces section. Surface faces are presentation and stay behind.
pub fn environment_volumes_from_level(
    world: &LevelWorld,
    resolve_fluid: impl Fn(&str) -> Option<u16>,
) -> EnvironmentVolumes {
    let volumes = world
        .environment_volumes
        .volumes
        .iter()
        .enumerate()
        .map(|(index, record)| EnvironmentVolume {
            source_entity: record.source_entity,
            priority: record.priority,
            gravity: record.gravity,
            push: record.push.map(Vec3::from),
            force: world.push_force(index),
            fluid: if record.fluid.is_empty() {
                None
            } else {
                resolve_fluid(&record.fluid)
            },
            brushes: record
                .brushes
                .iter()
                .map(|brush| ConvexLeaf {
                    planes: brush
                        .planes
                        .iter()
                        .map(|plane| LeafPlane {
                            normal: Vec3::from(plane.normal),
                            distance: plane.distance,
                        })
                        .collect(),
                    aabb_min: Vec3::from(brush.aabb_min),
                    aabb_max: Vec3::from(brush.aabb_max),
                })
                .collect(),
        })
        .collect();
    EnvironmentVolumes::build(volumes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_collision::{BodyProbe, EnvironmentView, resolve_environment};
    use postretro_level_format::environment_volumes::{
        EnvironmentBrush, EnvironmentPlane, EnvironmentVolumeRecord, EnvironmentVolumesSection,
    };
    use postretro_level_loader::{CellData, CellLocatorChild};

    fn box_brush(min: [f32; 3], max: [f32; 3]) -> EnvironmentBrush {
        let mut planes = Vec::new();
        for axis in 0..3 {
            let mut normal = [0.0; 3];
            normal[axis] = 1.0;
            planes.push(EnvironmentPlane {
                normal,
                distance: max[axis],
            });
            normal[axis] = -1.0;
            planes.push(EnvironmentPlane {
                normal,
                distance: -min[axis],
            });
        }
        EnvironmentBrush {
            aabb_min: min,
            aabb_max: max,
            planes,
        }
    }

    fn record(source_entity: u32, brush: EnvironmentBrush) -> EnvironmentVolumeRecord {
        EnvironmentVolumeRecord {
            source_entity,
            priority: 0,
            gravity: None,
            push: None,
            fluid: String::new(),
            brushes: vec![brush],
            surface_faces: Vec::new(),
        }
    }

    fn world_with(section: EnvironmentVolumesSection) -> LevelWorld {
        let mut world = LevelWorld::new_visibility_only(
            vec![CellData {
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
        .expect("single-cell fixture is visibility-valid");
        world.environment_volumes = section;
        world
    }

    fn probe(position: Vec3) -> BodyProbe {
        BodyProbe {
            position,
            half_height: 0.5,
            radius: 0.25,
            eye_height: 0.6,
        }
    }

    #[test]
    fn environment_volumes_from_level_keeps_every_field_and_map_order() {
        let mut low_gravity = record(7, box_brush([0.0; 3], [4.0; 3]));
        low_gravity.priority = 3;
        low_gravity.gravity = Some(-2.0);
        let mut wind = record(9, box_brush([10.0, 0.0, 0.0], [14.0, 4.0, 4.0]));
        wind.push = Some([1.0, 2.0, 3.0]);
        wind.fluid = "water".to_string();
        let world = world_with(EnvironmentVolumesSection {
            level_gravity: -9.81,
            volumes: vec![low_gravity, wind],
        });

        let volumes = environment_volumes_from_level(&world, |name| (name == "water").then_some(4));

        assert_eq!(volumes.len(), 2);
        let first = volumes.volume(0).unwrap();
        assert_eq!(
            (first.source_entity, first.priority, first.gravity),
            (7, 3, Some(-2.0))
        );
        assert_eq!((first.push, first.fluid), (None, None));
        let second = volumes.volume(1).unwrap();
        assert_eq!(second.source_entity, 9);
        assert_eq!(second.push, Some(Vec3::new(1.0, 2.0, 3.0)));
        assert_eq!(second.fluid, Some(4));
        assert_eq!(second.brushes[0].aabb_min, Vec3::new(10.0, 0.0, 0.0));
        assert_eq!(second.brushes[0].planes.len(), 6);

        let view = EnvironmentView::new(world.environment_volumes.level_gravity, &volumes, &[]);
        let inside = resolve_environment(view, &probe(Vec3::splat(2.0)));
        assert_eq!(inside.gravity, -2.0);
        let outside = resolve_environment(view, &probe(Vec3::new(7.0, 2.0, 2.0)));
        assert_eq!(outside.gravity, -9.81);
        let windy = resolve_environment(view, &probe(Vec3::new(12.0, 2.0, 2.0)));
        assert_eq!(windy.push, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn environment_volumes_from_level_loads_unresolved_fluid_names_dry() {
        let mut sludge = record(1, box_brush([0.0; 3], [1.0; 3]));
        sludge.fluid = "sludge".to_string();
        let dry = record(2, box_brush([0.0; 3], [1.0; 3]));
        let world = world_with(EnvironmentVolumesSection {
            level_gravity: -9.81,
            volumes: vec![sludge, dry],
        });
        let asked = std::cell::RefCell::new(Vec::new());

        let volumes = environment_volumes_from_level(&world, |name| {
            asked.borrow_mut().push(name.to_string());
            None
        });

        assert_eq!(volumes.volume(0).unwrap().fluid, None);
        assert_eq!(volumes.volume(1).unwrap().fluid, None);
        assert_eq!(
            asked.into_inner(),
            ["sludge"],
            "an empty fluid name is dry without asking the resolver"
        );
    }
}
