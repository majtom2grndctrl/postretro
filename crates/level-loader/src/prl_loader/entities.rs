// Entity section group: data script, map entities, kinematics, triggers, environment, fog, nav.
// See: context/lib/build_pipeline.md §PRL Compilation

use std::collections::{HashMap, HashSet};

use glam::Vec3;
use postretro_level_format::SectionId;
use postretro_level_format::data_script::DataScriptSection;
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::fog_cell_masks::FogCellMasksSection;
use postretro_level_format::fog_volumes::{FogVolumeRecord, FogVolumesSection};
use postretro_level_format::ghost_links::GhostLinksSection;
use postretro_level_format::kinematic_geometry::{
    KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH, KinematicGeometrySection,
};
use postretro_level_format::map_entity::{MapEntityRecord, MapEntitySection};
use postretro_level_format::navmesh::NavMeshSection;
use postretro_level_format::reflection_probes::ReflectionProbesSection;
use postretro_level_format::teleporters::TeleportersSection;
use postretro_level_format::trigger_volumes::{TriggerVolumeRecord, TriggerVolumesSection};

use super::{section_validation, section_validation_from_error, stale_section};
use crate::prl::{KinematicGeometry, LoadedKinematicWaypoint};
use crate::prl_container::PrlContainer;
use crate::{MapLight, PrlLoadError};

pub(crate) fn convert_kinematic_geometry_section(
    section: KinematicGeometrySection,
) -> Result<KinematicGeometry, PrlLoadError> {
    let geometry = KinematicGeometry {
        movers: section.movers.into_iter().map(Into::into).collect(),
        waypoints: section.waypoints.into_iter().map(Into::into).collect(),
    };
    validate_kinematic_geometry(&geometry)?;
    Ok(geometry)
}

/// Drop presentation-only mover associations that cannot address this map's
/// usable portal array. The loader keeps one warning for the whole section.
pub(super) fn drop_out_of_range_sealed_portal_ids(
    geometry: &mut KinematicGeometry,
    portal_count: usize,
) -> usize {
    let mut dropped_count = 0usize;
    for mover in &mut geometry.movers {
        let previous_len = mover.sealed_portal_ids.len();
        mover
            .sealed_portal_ids
            .retain(|&portal_id| (portal_id as usize) < portal_count);
        dropped_count += previous_len - mover.sealed_portal_ids.len();
    }
    dropped_count
}

/// Drop presentation-only light associations that cannot resolve to exactly
/// one dynamic AlphaLights record or compose to a runtime-finite spawn pose.
/// Invalid content leaves the light unbound.
pub(super) fn drop_invalid_carried_light_links(
    geometry: &mut KinematicGeometry,
    lights: &[MapLight],
) -> usize {
    let mut reference_counts = HashMap::<u32, usize>::new();
    for member in geometry
        .movers
        .iter()
        .flat_map(|mover| mover.carried_lights.iter())
    {
        *reference_counts
            .entry(member.alpha_light_index)
            .or_default() += 1;
    }

    let duplicate_indices: HashSet<u32> = reference_counts
        .iter()
        .filter_map(|(&alpha_light_index, &count)| (count > 1).then_some(alpha_light_index))
        .collect();
    for &alpha_light_index in &duplicate_indices {
        let count = reference_counts[&alpha_light_index];
        log::warn!(
            "[PRL] KinematicGeometry: dropped {count} duplicate carried-light references to AlphaLight {alpha_light_index}; a light may have only one carrier"
        );
    }

    let mut dropped_count = 0usize;
    for mover in &mut geometry.movers {
        let mover_id = mover.mover_id;
        let mover_name = mover.name.clone();
        mover.carried_lights.retain(|member| {
            if duplicate_indices.contains(&member.alpha_light_index) {
                dropped_count += 1;
                return false;
            }
            let Some(light) = lights.get(member.alpha_light_index as usize) else {
                log::warn!(
                    "[PRL] KinematicGeometry: dropped mover {mover_id} (`{mover_name}`) carried-light reference to AlphaLight {} outside the {} loaded lights",
                    member.alpha_light_index,
                    lights.len(),
                );
                dropped_count += 1;
                return false;
            };
            if !light.is_dynamic {
                log::warn!(
                    "[PRL] KinematicGeometry: dropped mover {mover_id} (`{mover_name}`) carried-light reference to non-dynamic AlphaLight {}",
                    member.alpha_light_index,
                );
                dropped_count += 1;
                return false;
            }
            let composed_position = [
                f64::from(mover.origin.x) + f64::from(member.local_offset.x),
                f64::from(mover.origin.y) + f64::from(member.local_offset.y),
                f64::from(mover.origin.z) + f64::from(member.local_offset.z),
            ];
            if composed_position
                .iter()
                .any(|component| {
                    !component.is_finite() || component.abs() > f64::from(f32::MAX)
                })
            {
                log::warn!(
                    "[PRL] KinematicGeometry: dropped mover {mover_id} (`{mover_name}`) carried-light reference to AlphaLight {} because its authored position is outside the runtime f32 range",
                    member.alpha_light_index,
                );
                dropped_count += 1;
                return false;
            }
            true
        });
    }
    dropped_count
}

fn validate_kinematic_geometry(geometry: &KinematicGeometry) -> Result<(), PrlLoadError> {
    let mut waypoints: HashMap<&str, usize> = HashMap::new();
    for (index, waypoint) in geometry.waypoints.iter().enumerate() {
        if waypoint.name.is_empty() {
            return Err(section_validation(
                "KinematicGeometry",
                format!("waypoint {index} has an empty name"),
            ));
        }
        if !waypoint.origin.is_finite() {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "waypoint `{}` has non-finite origin {:?}",
                    waypoint.name, waypoint.origin
                ),
            ));
        }
        if waypoints.insert(waypoint.name.as_str(), index).is_some() {
            return Err(section_validation(
                "KinematicGeometry",
                format!("duplicate waypoint name `{}`", waypoint.name),
            ));
        }
    }

    let mut mover_ids = HashSet::new();
    for mover in &geometry.movers {
        if !mover_ids.insert(mover.mover_id) {
            return Err(section_validation(
                "KinematicGeometry",
                format!("duplicate mover_id {}", mover.mover_id),
            ));
        }
        if mover.vertices.is_empty() || mover.indices.is_empty() {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "mover {} (`{}`) geometry must contain vertices and indices",
                    mover.mover_id, mover.name
                ),
            ));
        }
        if !mover.origin.is_finite() {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "mover {} (`{}`) has non-finite origin {:?}",
                    mover.mover_id, mover.name, mover.origin
                ),
            ));
        }
        let resolved = resolve_kinematic_waypoint_chain(
            mover.mover_id,
            &mover.name,
            &mover.path,
            &geometry.waypoints,
            &waypoints,
        )?;
        if mover.spin_speed_deg_s != 0.0 && mover.spin_speed_deg_s.to_radians() == 0.0 {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "mover {} (`{}`) has nonzero spin_speed_deg_s that becomes zero after conversion to radians/sec",
                    mover.mover_id, mover.name
                ),
            ));
        }
        if mover.spin_accel_deg_s2 > 0.0 && mover.spin_accel_deg_s2.to_radians() == 0.0 {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "mover {} (`{}`) has positive spin_accel_deg_s2 that becomes zero after conversion to radians/sec²",
                    mover.mover_id, mover.name
                ),
            ));
        }
        if mover.spin_speed_deg_s != 0.0 && mover.spin_axis.normalize_or_zero() == Vec3::ZERO {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "mover {} (`{}`) has nonzero spin_speed_deg_s but spin_axis normalizes to zero",
                    mover.mover_id, mover.name
                ),
            ));
        }
        let allow_single_waypoint = mover.spin_speed_deg_s != 0.0;
        if resolved.len() < 2 && !allow_single_waypoint {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "mover {} (`{}`) path `{}` resolves to {} waypoint(s); at least 2 required",
                    mover.mover_id,
                    mover.name,
                    mover.path,
                    resolved.len()
                ),
            ));
        }
        if (mover.origin - resolved[0]).length() > KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "mover {} (`{}`) origin {:?} must match first waypoint `{}` at {:?}",
                    mover.mover_id, mover.name, mover.origin, mover.path, resolved[0]
                ),
            ));
        }
    }

    Ok(())
}

fn resolve_kinematic_waypoint_chain(
    mover_id: u32,
    mover_name: &str,
    path: &str,
    waypoints: &[LoadedKinematicWaypoint],
    waypoint_indices: &HashMap<&str, usize>,
) -> Result<Vec<Vec3>, PrlLoadError> {
    if path.is_empty() {
        return Err(section_validation(
            "KinematicGeometry",
            format!("mover {mover_id} (`{mover_name}`) has an empty path"),
        ));
    }

    let mut positions: Vec<Vec3> = Vec::new();
    let mut seen = HashSet::new();
    let mut current = path;
    loop {
        if !seen.insert(current.to_string()) {
            return Err(section_validation(
                "KinematicGeometry",
                format!("mover {mover_id} (`{mover_name}`) waypoint chain cycles at `{current}`"),
            ));
        }
        let Some(&index) = waypoint_indices.get(current) else {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "mover {mover_id} (`{mover_name}`) references unknown waypoint `{current}`"
                ),
            ));
        };
        let waypoint = &waypoints[index];
        if let Some(previous) = positions.last()
            && (*previous - waypoint.origin).length() <= KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH
        {
            return Err(section_validation(
                "KinematicGeometry",
                format!(
                    "mover {mover_id} (`{mover_name}`) path has zero-length segment ending at waypoint `{}`",
                    waypoint.name
                ),
            ));
        }
        positions.push(waypoint.origin);
        if waypoint.next.is_empty() {
            break;
        }
        current = waypoint.next.as_str();
    }
    Ok(positions)
}

pub(super) struct EntityDecode {
    pub(super) data_script: Option<DataScriptSection>,
    pub(super) map_entities: Vec<MapEntityRecord>,
    pub(super) kinematic_geometry: KinematicGeometry,
    pub(super) trigger_volumes: Vec<TriggerVolumeRecord>,
    pub(super) teleporters: TeleportersSection,
    pub(super) ghost_links: GhostLinksSection,
    pub(super) environment_volumes: EnvironmentVolumesSection,
    pub(super) environment_push_forces: Vec<f32>,
    pub(super) reflection_probes: Option<ReflectionProbesSection>,
    pub(super) fog_volumes: Vec<FogVolumeRecord>,
    pub(super) fog_pixel_scale: u32,
    pub(super) fog_cell_masks: Option<Vec<u32>>,
    pub(super) navmesh: Option<NavMeshSection>,
}

impl EntityDecode {
    /// Hand-out 4: the decoded entity group plus the cell draw index. The
    /// fog cell masks are not part of it: parse-end cross-section validation
    /// owns them, so they ride hand-out 5. Take them out first.
    pub(super) fn into_hand_out(
        self,
        cell_draw_index: Option<crate::CellDrawIndex>,
    ) -> super::staged::EntitiesHandOut {
        let EntityDecode {
            data_script,
            map_entities,
            kinematic_geometry,
            trigger_volumes,
            teleporters,
            ghost_links,
            environment_volumes,
            environment_push_forces,
            reflection_probes,
            fog_volumes,
            fog_pixel_scale,
            fog_cell_masks,
            navmesh,
        } = self;
        debug_assert!(
            fog_cell_masks.is_none(),
            "fog cell masks leave the entity group before hand-out 4"
        );
        super::staged::EntitiesHandOut {
            data_script,
            map_entities,
            kinematic_geometry,
            trigger_volumes,
            teleporters,
            ghost_links,
            environment_volumes,
            environment_push_forces,
            reflection_probes,
            fog_volumes,
            fog_pixel_scale,
            navmesh,
            cell_draw_index,
        }
    }
}

/// Decodes the entity group. `usable_portal_count` is the length of the
/// usable portal array spatial decode produced (`None` when the level has no
/// usable portals): mover sealed-portal ids it cannot address are dropped
/// here, so the kinematic geometry leaves this decode final.
pub(super) fn decode_entities(
    container: &PrlContainer,
    lights: &[MapLight],
    usable_portal_count: Option<usize>,
) -> Result<EntityDecode, PrlLoadError> {
    let read_section = |section: SectionId| container.read_section(section as u32);

    // Optional — absent when map has no `data_script` worldspawn KVP.
    let data_script: Option<DataScriptSection> = match read_section(SectionId::DataScript)? {
        Some(data) => {
            let section = DataScriptSection::from_bytes(&data)?;
            log::info!(
                "[PRL] DataScript: {} bytes from `{}`",
                section.compiled_bytes.len(),
                section.source_path,
            );
            Some(section)
        }
        None => None,
    };

    // Optional — absent when no non-light, non-worldspawn entities exist.
    let map_entities: Vec<MapEntityRecord> = match read_section(SectionId::MapEntity)? {
        Some(data) => {
            let section = MapEntitySection::from_bytes(&data)?;
            log::info!("[PRL] MapEntity: {} entities", section.entries.len());
            section.entries
        }
        None => Vec::new(),
    };

    // Optional — absent or empty means the level has no kinematic movers.
    let mut kinematic_geometry: KinematicGeometry =
        match read_section(SectionId::KinematicGeometry)? {
            Some(data) => {
                let section = KinematicGeometrySection::from_bytes(&data)
                    .map_err(|err| section_validation_from_error("KinematicGeometry", err))?;
                convert_kinematic_geometry_section(section)?
            }
            None => KinematicGeometry::default(),
        };
    drop_invalid_carried_light_links(&mut kinematic_geometry, lights);
    // Only the usable array's length decides this drop, so it runs here
    // rather than beside the parse-end adjacency check that the array is
    // otherwise validated by.
    if let Some(portal_count) = usable_portal_count {
        let dropped_count =
            drop_out_of_range_sealed_portal_ids(&mut kinematic_geometry, portal_count);
        if dropped_count > 0 {
            log::warn!(
                "[PRL] KinematicGeometry: dropped {dropped_count} sealed portal id(s) outside the {portal_count} loaded portals",
            );
        }
    }
    let kinematic_vertex_count: usize = kinematic_geometry
        .movers
        .iter()
        .map(|mover| mover.vertices.len())
        .sum();
    let kinematic_index_count: usize = kinematic_geometry
        .movers
        .iter()
        .map(|mover| mover.indices.len())
        .sum();
    log::info!(
        "[PRL] KinematicGeometry: {} movers, {} waypoints, {} vertices, {} indices",
        kinematic_geometry.movers.len(),
        kinematic_geometry.waypoints.len(),
        kinematic_vertex_count,
        kinematic_index_count,
    );
    // Optional — absent or empty means no trigger volumes.
    let trigger_volumes = match read_section(SectionId::TriggerVolumes)? {
        Some(data) => {
            TriggerVolumesSection::from_bytes(&data)
                .map_err(|err| section_validation_from_error("TriggerVolumes", err))?
                .triggers
        }
        None => Vec::new(),
    };
    let teleporters =
        crate::prl_teleporters::decode_teleporters(read_section(SectionId::Teleporters)?)?;
    let ghost_links =
        crate::prl_ghost_links::decode_ghost_links(read_section(SectionId::GhostLinks)?)?;
    let touch_trigger_count = trigger_volumes
        .iter()
        .filter(|trigger| trigger.activation == 0)
        .count();
    let use_trigger_count = trigger_volumes
        .iter()
        .filter(|trigger| trigger.activation == 1)
        .count();
    let start_command_count = trigger_volumes
        .iter()
        .filter(|trigger| trigger.command == 0)
        .count();
    let stop_command_count = trigger_volumes
        .iter()
        .filter(|trigger| trigger.command == 1)
        .count();
    let reverse_command_count = trigger_volumes
        .iter()
        .filter(|trigger| trigger.command == 2)
        .count();
    let go_to_path_node_command_count = trigger_volumes
        .iter()
        .filter(|trigger| trigger.command == 3)
        .count();
    let fire_command_count = trigger_volumes
        .iter()
        .filter(|trigger| trigger.command == 4)
        .count();
    log::info!(
        "[PRL] TriggerVolumes: {} trigger(s): {touch_trigger_count} touch, {use_trigger_count} use; commands: {start_command_count} start, {stop_command_count} stop, {reverse_command_count} reverse, {go_to_path_node_command_count} go_to_path_node, {fire_command_count} fire",
        trigger_volumes.len(),
    );

    // Required, and checked before FogVolumes: it carries the level default
    // gravity, and its absence marks a PRL whose FogVolumes uses the retired
    // gravity-carrying layout, which must fail with the recompile message.
    let environment_volumes = crate::prl_environment::decode_environment_volumes(read_section(
        SectionId::EnvironmentVolumes,
    )?)?;
    let environment_push_forces = crate::prl_environment::decode_push_forces(
        read_section(SectionId::PushForces)?,
        &environment_volumes,
    )?;
    let reflection_probes = crate::prl_environment::decode_reflection_probes(
        read_section(SectionId::ReflectionProbes)?,
        &environment_volumes,
    )?;

    // Required — carries the worldspawn fog pixel scale even with no fog volumes.
    let (fog_volumes, fog_pixel_scale): (Vec<FogVolumeRecord>, u32) =
        match read_section(SectionId::FogVolumes)? {
            Some(data) => {
                let section = FogVolumesSection::from_bytes(&data)
                    .map_err(|err| section_validation_from_error("FogVolumes", err))?;
                log::info!(
                    "[PRL] FogVolumes: {} volumes, pixel_scale={}",
                    section.volumes.len(),
                    section.pixel_scale,
                );
                (section.volumes, section.pixel_scale)
            }
            None => return Err(stale_section("FogVolumes", SectionId::FogVolumes)),
        };

    // Required when FogVolumes contains canonical fog entities; optional only
    // for no-fog maps, where `compute_fog_cell_mask` can keep all zero slots.
    let fog_cell_masks: Option<Vec<u32>> = match read_section(SectionId::FogCellMasks)? {
        Some(data) => {
            let section = FogCellMasksSection::from_bytes(&data)
                .map_err(|err| section_validation_from_error("FogCellMasks", err))?;
            log::info!("[PRL] FogCellMasks: {} cells", section.masks.len());
            Some(section.masks)
        }
        None => None,
    };

    // Optional — absent → no runtime navigation (logged at info, mirroring the
    // SdfAtlas precedent for the absent-section case). A malformed body warns
    // and decodes to None (softer than SdfAtlas, which propagates with `?` and
    // fails the load): nothing depends on the navmesh yet, so warn-and-continue
    // is intentional rather than making a malformed navmesh unplayable.
    let navmesh: Option<NavMeshSection> = match read_section(SectionId::NavMesh)? {
        Some(data) => match NavMeshSection::from_bytes(&data) {
            Ok(section) => {
                log::info!(
                    "[PRL] NavMesh: {}×{} grid, cell_size={:.4}m, {} region(s), {} portal(s)",
                    section.dim_x,
                    section.dim_z,
                    section.cell_size,
                    section.regions.len(),
                    section.portals.len(),
                );
                Some(section)
            }
            Err(err) => {
                log::warn!("[PRL] NavMesh section malformed, ignoring: {err}");
                None
            }
        },
        None => {
            log::info!("[PRL] NavMesh section missing — no runtime navigation for this map");
            None
        }
    };

    Ok(EntityDecode {
        data_script,
        map_entities,
        kinematic_geometry,
        trigger_volumes,
        teleporters,
        ghost_links,
        environment_volumes,
        environment_push_forces,
        reflection_probes,
        fog_volumes,
        fog_pixel_scale,
        fog_cell_masks,
        navmesh,
    })
}
