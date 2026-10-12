// Canonical environment volumes → PRL EnvironmentVolumes section (id 52).
// See: context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::environment_volumes::{
    EnvironmentBrush, EnvironmentPlane, EnvironmentVolumeRecord, EnvironmentVolumesSection,
};

use crate::map_data::MapEnvironmentVolume;

/// Encode the always-present section: the level default gravity plus every
/// volume in source map order. Surface faces are not built yet, so every
/// volume carries none.
pub fn encode_environment_volumes_section(
    volumes: &[MapEnvironmentVolume],
    level_gravity: f32,
) -> EnvironmentVolumesSection {
    EnvironmentVolumesSection {
        level_gravity,
        volumes: volumes
            .iter()
            .map(|volume| EnvironmentVolumeRecord {
                source_entity: volume.source_entity,
                priority: volume.priority,
                gravity: volume.gravity,
                push: volume.push.map(|push| push.to_array()),
                fluid: volume.fluid.clone().unwrap_or_default(),
                brushes: volume
                    .brushes
                    .iter()
                    .map(|brush| EnvironmentBrush {
                        aabb_min: brush.min,
                        aabb_max: brush.max,
                        planes: brush
                            .planes
                            .iter()
                            .map(|&[nx, ny, nz, distance]| EnvironmentPlane {
                                normal: [nx, ny, nz],
                                distance,
                            })
                            .collect(),
                    })
                    .collect(),
                surface_faces: Vec::new(),
            })
            .collect(),
    }
}
