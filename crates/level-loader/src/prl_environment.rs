// EnvironmentVolumes (id 52) decode, and its optional PushForces (id 53) and
// ReflectionProbes (id 54) siblings.
// See: context/lib/build_pipeline.md §PRL section IDs

use postretro_level_format::SectionId;
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::push_forces::PushForcesSection;
use postretro_level_format::reflection_probes::ReflectionProbesSection;

use crate::prl::{LevelWorld, PrlLoadError};
use crate::prl_loader::section_validation_from_error;

/// Decode the always-present environment section. Absence means the PRL was
/// built before level gravity left FogVolumes, whose old layout the current
/// FogVolumes decoder cannot read; the caller checks this first so such a file
/// fails with the recompile message rather than a FogVolumes layout error.
pub(crate) fn decode_environment_volumes(
    data: Option<Vec<u8>>,
) -> Result<EnvironmentVolumesSection, PrlLoadError> {
    let Some(data) = data else {
        return Err(PrlLoadError::StaleFormatMissingSection {
            section: "EnvironmentVolumes",
            id: SectionId::EnvironmentVolumes as u32,
        });
    };
    let section = EnvironmentVolumesSection::from_bytes(&data)
        .map_err(|err| section_validation_from_error("EnvironmentVolumes", err))?;
    log::info!(
        "[PRL] EnvironmentVolumes: {} volume(s), {} brush(es), level_gravity={}",
        section.volumes.len(),
        section
            .volumes
            .iter()
            .map(|volume| volume.brushes.len())
            .sum::<usize>(),
        section.level_gravity,
    );
    Ok(section)
}

/// Decode the optional PushForces section against its records. Absent means
/// every force is 1 and yields an empty list. Present, it holds one finite
/// force >= 0 per record, and a record without push carries exactly 1.
pub(crate) fn decode_push_forces(
    data: Option<Vec<u8>>,
    environment: &EnvironmentVolumesSection,
) -> Result<Vec<f32>, PrlLoadError> {
    let Some(data) = data else {
        return Ok(Vec::new());
    };
    let section = PushForcesSection::from_bytes(&data)
        .map_err(|err| section_validation_from_error("PushForces", err))?;
    if section.forces.len() != environment.volumes.len() {
        return Err(section_validation_from_error(
            "PushForces",
            format!(
                "{} forces for {} environment volumes",
                section.forces.len(),
                environment.volumes.len()
            ),
        ));
    }
    for (index, (force, record)) in section.forces.iter().zip(&environment.volumes).enumerate() {
        if record.push.is_none() && *force != 1.0 {
            return Err(section_validation_from_error(
                "PushForces",
                format!("volume {index} sets no push but carries force {force}"),
            ));
        }
    }
    log::info!(
        "[PRL] PushForces: {} of {} volume(s) scaled",
        section.forces.iter().filter(|force| **force != 1.0).count(),
        section.forces.len(),
    );
    // `-0.0` passes the `>= 0` check and behaves as 0, but the digest hashes its
    // bits apart from `+0.0`; the compiler never writes it, a hand-edited PRL can.
    Ok(section
        .forces
        .into_iter()
        .map(|force| if force == 0.0 { 0.0 } else { force })
        .collect())
}

/// Decode the optional ReflectionProbes section and check its bindings name
/// real surface faces. Absent means no probes. The roster references are
/// checked once lighting is loaded ([`validate_reflection_probe_lights`]).
pub(crate) fn decode_reflection_probes(
    data: Option<Vec<u8>>,
    environment: &EnvironmentVolumesSection,
) -> Result<Option<ReflectionProbesSection>, PrlLoadError> {
    let Some(data) = data else {
        return Ok(None);
    };
    let section = ReflectionProbesSection::from_bytes(&data)
        .map_err(|err| section_validation_from_error("ReflectionProbes", err))?;
    for binding in &section.bindings {
        let Some(volume) = environment.volumes.get(binding.volume as usize) else {
            return Err(section_validation_from_error(
                "ReflectionProbes",
                format!(
                    "binding names volume {}, past the {} environment volumes",
                    binding.volume,
                    environment.volumes.len()
                ),
            ));
        };
        if binding.face as usize >= volume.surface_faces.len() {
            return Err(section_validation_from_error(
                "ReflectionProbes",
                format!(
                    "binding names volume {} face {}, past its {} surface faces",
                    binding.volume,
                    binding.face,
                    volume.surface_faces.len()
                ),
            ));
        }
    }
    log::info!(
        "[PRL] ReflectionProbes: {} probe(s), {}² tiles, {} bound face(s)",
        section.probes.len(),
        section.tile_dimension,
        section.bindings.len(),
    );
    Ok(Some(section))
}

/// A share entry names a light by its AnimatedBakedLights roster index and
/// the fluid shader reads that index straight into the descriptor buffer, so
/// every entry must fall inside the roster and both roster-to-descriptor
/// tables must be the identity the compiler writes. Identity is checked over
/// each table's own length only: a table shorter or longer than the roster
/// passes here, where the compiler's `check_roster` refuses it at build time.
///
/// The fluid stage binds the descriptors but no index table, so it reads
/// descriptor `r` for roster `r`. Each compose pass maps roster `r` through
/// its own table: SH compose (the form the fluid ratio ports) through
/// DeltaShVolumes (id 27), whose roster the compiler numbers the shares in,
/// and animated direct compose through AnimatedDirectShDeltaVolumes (id 45).
/// A non-identity id 27 animates a share by another light's curve; a
/// non-identity id 45 animates the world's direct term for that light by a
/// different descriptor than its reflection.
pub(crate) fn validate_reflection_probe_lights(world: &LevelWorld) -> Result<(), PrlLoadError> {
    let Some(section) = world.reflection_probes.as_ref() else {
        return Ok(());
    };
    let roster_len = world.animation_descriptors().len();
    for (p, probe) in section.probes.iter().enumerate() {
        for light in probe.lights.iter().filter(|light| light.is_used()) {
            if light.roster_index as usize >= roster_len {
                return Err(section_validation_from_error(
                    "ReflectionProbes",
                    format!(
                        "probe {p} names animated light {}, past the {roster_len}-light roster",
                        light.roster_index
                    ),
                ));
            }
        }
    }
    for (table, indices) in [
        (
            "DeltaShVolumes (id 27)",
            indirect_delta_descriptor_indices(world),
        ),
        (
            "AnimatedDirectShDeltaVolumes (id 45)",
            world.animated_direct_descriptor_indices(),
        ),
    ] {
        if let Some((slot, descriptor)) = indices
            .iter()
            .enumerate()
            .find(|(slot, descriptor)| **descriptor as usize != *slot)
        {
            return Err(section_validation_from_error(
                "ReflectionProbes",
                format!(
                    "{table} maps animated light {slot} to descriptor {descriptor}; probe shares \
                     need the identity roster"
                ),
            ));
        }
    }
    Ok(())
}

/// Id 27's roster-to-descriptor table, from the streaming metadata or the
/// resident section; empty when the level has no indirect delta.
fn indirect_delta_descriptor_indices(world: &LevelWorld) -> &[u32] {
    match world.sh_stream_manifest() {
        Some(manifest) => manifest
            .sources()
            .indirect_delta
            .as_ref()
            .map_or(&[], |metadata| &metadata.animation_descriptor_indices),
        None => world
            .delta_sh_volumes()
            .map_or(&[], |section| &section.animation_descriptor_indices),
    }
}
